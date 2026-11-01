#!/usr/bin/env bash
# Egress allowlist for the dev container.
# Adapted from Anthropic's reference devcontainer:
#   https://github.com/anthropics/claude-code/blob/main/.devcontainer/init-firewall.sh
# Differences: domain list read from a file baked into the image, IPv6 egress
# blocked, unresolvable domains warn instead of aborting, no `aggregate` dep,
# safe to re-run, and a --refresh mode for CDN-hosted domains.
#
# Usage (as root; the dev user may run it via sudo):
#   init-firewall.sh            Set up (or rebuild) the firewall. Safe to re-run:
#                               everything is collected before the rules change,
#                               and the rules are swapped in atomically, so a
#                               failure leaves the previous firewall in place.
#   init-firewall.sh --refresh  Add the *current* IPs of the allowlisted domains
#                               to the allowed set. Never removes or loosens
#                               anything; a no-op if the firewall isn't set up.
#
# Why --refresh exists: the firewall allows IPs, resolved from domain names.
# CDN-hosted domains (crates.io and static.crates.io on Fastly, docs.rs, ...)
# rotate through many IPs with a ~60s DNS TTL, so a one-time lookup goes stale.
# post-start.sh runs --refresh every 30s, and the set accumulates the CDN's
# rotation pool. GitHub isn't affected: its published ranges are added instead.
set -euo pipefail
IFS=$'\n\t'

ALLOWLIST=/usr/local/etc/allowed-domains.txt
SET=allowed-domains
NEW_SET=allowed-domains-new

# The sudoers rule allows any arguments, so accept only the known ones.
MODE=setup
case "${1:-}" in
    "") ;;
    --refresh) MODE=refresh ;;
    *)
        echo "usage: $0 [--refresh]" >&2
        exit 2
        ;;
esac
if [ $# -gt 1 ]; then
    echo "usage: $0 [--refresh]" >&2
    exit 2
fi

# IPv4 addresses (one per line) that `domain` resolves to right now.
resolve() {
    dig +noall +answer A "$1" | awk '$4 == "A" {print $5}' |
        grep -E '^[0-9]{1,3}(\.[0-9]{1,3}){3}$' || true
}

# Domains from the allowlist file, comments and blanks stripped.
allowlisted_domains() {
    while read -r domain; do
        domain="${domain%%#*}"
        domain="${domain//[[:space:]]/}"
        [ -n "$domain" ] && echo "$domain"
    done < "$ALLOWLIST"
}

# ---------------------------------------------------------------------------
# --refresh: add newly resolved IPs, print only what changed (keeps logs small).
# ---------------------------------------------------------------------------
if [ "$MODE" = refresh ]; then
    if ! ipset list -n "$SET" >/dev/null 2>&1; then
        exit 0 # firewall not set up (e.g. DEVCONTAINER_FIREWALL=0): nothing to do
    fi
    for domain in $(allowlisted_domains); do
        for ip in $(resolve "$domain"); do
            if ! ipset test "$SET" "$ip" >/dev/null 2>&1; then
                ipset add -exist "$SET" "$ip"
                echo "$(date '+%F %T') refresh: allowed $ip ($domain)"
            fi
        done
    done
    exit 0
fi

# ---------------------------------------------------------------------------
# Setup. Steps 1-3 only *add* narrow temporary rules, so that this works from
# any starting state: no firewall yet, a working one, or a broken one left by
# an older version of this script (default-deny with its rules flushed).
# ---------------------------------------------------------------------------

# 1. Temporarily allow what collecting the allowlist needs: DNS, replies, and
#    HTTPS to api.github.com. Removed on exit; step 4 replaces the whole table.
TEMP_RULES=()
# iptables_rule -I|-D "CHAIN match... -j TARGET": split the rule on spaces
# (a prefix `IFS=' ' cmd $rule` would not: expansion uses the current IFS).
iptables_rule() {
    local IFS=' '
    # shellcheck disable=SC2086 # the rule is meant to split into arguments
    iptables "$1" $2
}
add_temp_rule() {
    iptables_rule -I "$1"
    TEMP_RULES+=("$1")
}
remove_temp_rules() {
    local rule
    for rule in "${TEMP_RULES[@]}"; do
        iptables_rule -D "$rule" 2>/dev/null || true
    done
}
trap remove_temp_rules EXIT
add_temp_rule "OUTPUT -p udp --dport 53 -j ACCEPT"
add_temp_rule "INPUT -p udp --sport 53 -j ACCEPT"
add_temp_rule "INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT"
for ip in $(resolve api.github.com); do # needs the DNS rules above
    add_temp_rule "OUTPUT -d $ip -p tcp --dport 443 -j ACCEPT"
done

# 2. Collect the allowed addresses into a new set, while the old one stays live.
ipset destroy "$NEW_SET" 2>/dev/null || true
ipset create "$NEW_SET" hash:net

echo "Fetching GitHub IP ranges..."
gh_ranges=$(curl -s --connect-timeout 10 https://api.github.com/meta)
if ! echo "$gh_ranges" | jq -e '.web and .api and .git' >/dev/null 2>&1; then
    echo "ERROR: could not fetch GitHub IP ranges; previous firewall left in place" >&2
    ipset destroy "$NEW_SET" 2>/dev/null || true
    exit 1
fi
while read -r cidr; do
    if [[ "$cidr" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}/[0-9]{1,2}$ ]]; then
        ipset add -exist "$NEW_SET" "$cidr"
    fi
done < <(echo "$gh_ranges" | jq -r '(.web + .api + .git)[]' | sort -u)

for domain in $(allowlisted_domains); do
    ips=$(resolve "$domain")
    if [ -z "$ips" ]; then
        echo "WARNING: failed to resolve $domain, skipping" >&2
        continue
    fi
    for ip in $ips; do
        ipset add -exist "$NEW_SET" "$ip"
    done
    echo "Allowed $domain"
done

# Docker host network (port forwarding, host services on the bridge).
HOST_IP=$(ip route | awk '/default/ {print $3; exit}')
if [ -z "$HOST_IP" ]; then
    echo "ERROR: failed to detect host IP; previous firewall left in place" >&2
    ipset destroy "$NEW_SET" 2>/dev/null || true
    exit 1
fi
HOST_NETWORK="${HOST_IP%.*}.0/24"
echo "Host network: $HOST_NETWORK"

# 3. Swap the new set in under the name the rules use.
if ipset list -n "$SET" >/dev/null 2>&1; then
    ipset swap "$NEW_SET" "$SET"
    ipset destroy "$NEW_SET"
else
    ipset rename "$NEW_SET" "$SET"
fi

# 4. Replace the whole filter table in one atomic step (this also drops the
#    temporary rules). The nat table is left alone: Docker's embedded DNS
#    rules live there.
iptables-restore <<EOF
*filter
:INPUT DROP [0:0]
:FORWARD DROP [0:0]
:OUTPUT DROP [0:0]
-A INPUT -i lo -j ACCEPT
-A OUTPUT -o lo -j ACCEPT
-A OUTPUT -p udp --dport 53 -j ACCEPT
-A INPUT -p udp --sport 53 -j ACCEPT
-A OUTPUT -p tcp --dport 22 -j ACCEPT
-A INPUT -p tcp --sport 22 -m state --state ESTABLISHED -j ACCEPT
-A INPUT -s $HOST_NETWORK -j ACCEPT
-A OUTPUT -d $HOST_NETWORK -j ACCEPT
-A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
-A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
-A OUTPUT -m set --match-set $SET dst -j ACCEPT
-A OUTPUT -j REJECT --reject-with icmp-admin-prohibited
COMMIT
EOF
trap - EXIT # the temporary rules are gone with the old table

# IPv6: allow loopback only, so v6 can't bypass the v4 allowlist.
if command -v ip6tables-restore >/dev/null && ip6tables -L >/dev/null 2>&1; then
    ip6tables-restore <<EOF
*filter
:INPUT DROP [0:0]
:FORWARD DROP [0:0]
:OUTPUT DROP [0:0]
-A INPUT -i lo -j ACCEPT
-A OUTPUT -o lo -j ACCEPT
COMMIT
EOF
fi

# 5. Verify.
if curl -s --connect-timeout 5 https://example.com >/dev/null 2>&1; then
    echo "ERROR: firewall verification failed, example.com is reachable" >&2
    exit 1
fi
if ! curl -s -o /dev/null --connect-timeout 5 https://api.github.com/zen; then
    echo "ERROR: firewall verification failed, api.github.com is unreachable" >&2
    exit 1
fi
if ! curl -s -o /dev/null --connect-timeout 5 https://api.anthropic.com; then
    echo "WARNING: api.anthropic.com is unreachable; Claude Code will not work" >&2
fi
echo "Firewall ready: outbound traffic limited to $ALLOWLIST + GitHub."
