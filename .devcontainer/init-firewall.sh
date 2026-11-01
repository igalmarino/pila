#!/usr/bin/env bash
# Egress allowlist for the dev container.
# Adapted from Anthropic's reference devcontainer:
#   https://github.com/anthropics/claude-code/blob/main/.devcontainer/init-firewall.sh
# Differences: domain list read from a file baked into the image, IPv6 egress
# blocked, unresolvable domains warn instead of aborting, no `aggregate` dep.
set -euo pipefail
IFS=$'\n\t'

ALLOWLIST=/usr/local/etc/allowed-domains.txt

# 1. Save Docker's embedded DNS NAT rules before flushing.
DOCKER_DNS_RULES=$(iptables-save -t nat | grep "127\.0\.0\.11" || true)

iptables -F
iptables -X
iptables -t nat -F
iptables -t nat -X
iptables -t mangle -F
iptables -t mangle -X
ipset destroy allowed-domains 2>/dev/null || true

if [ -n "$DOCKER_DNS_RULES" ]; then
    echo "Restoring Docker DNS rules..."
    iptables -t nat -N DOCKER_OUTPUT 2>/dev/null || true
    iptables -t nat -N DOCKER_POSTROUTING 2>/dev/null || true
    echo "$DOCKER_DNS_RULES" | xargs -L 1 iptables -t nat
else
    echo "No Docker DNS rules to restore"
fi

# 2. Always allow DNS, SSH (git over ssh) and loopback.
iptables -A OUTPUT -p udp --dport 53 -j ACCEPT
iptables -A INPUT -p udp --sport 53 -j ACCEPT
iptables -A OUTPUT -p tcp --dport 22 -j ACCEPT
iptables -A INPUT -p tcp --sport 22 -m state --state ESTABLISHED -j ACCEPT
iptables -A INPUT -i lo -j ACCEPT
iptables -A OUTPUT -o lo -j ACCEPT

ipset create allowed-domains hash:net

# 3. GitHub ranges.
echo "Fetching GitHub IP ranges..."
gh_ranges=$(curl -s --connect-timeout 10 https://api.github.com/meta)
if ! echo "$gh_ranges" | jq -e '.web and .api and .git' >/dev/null 2>&1; then
    echo "ERROR: could not fetch GitHub IP ranges" >&2
    exit 1
fi
while read -r cidr; do
    if [[ "$cidr" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}/[0-9]{1,2}$ ]]; then
        ipset add -exist allowed-domains "$cidr"
    fi
done < <(echo "$gh_ranges" | jq -r '(.web + .api + .git)[]' | sort -u)

# 4. Domains from the allowlist file.
while read -r domain; do
    domain="${domain%%#*}"
    domain="${domain//[[:space:]]/}"
    [ -z "$domain" ] && continue
    ips=$(dig +noall +answer A "$domain" | awk '$4 == "A" {print $5}')
    if [ -z "$ips" ]; then
        echo "WARNING: failed to resolve $domain, skipping" >&2
        continue
    fi
    while read -r ip; do
        if [[ "$ip" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]]; then
            ipset add -exist allowed-domains "$ip"
        fi
    done <<< "$ips"
    echo "Allowed $domain"
done < "$ALLOWLIST"

# 5. Docker host network (port forwarding, host services on the bridge).
HOST_IP=$(ip route | awk '/default/ {print $3; exit}')
if [ -z "$HOST_IP" ]; then
    echo "ERROR: failed to detect host IP" >&2
    exit 1
fi
HOST_NETWORK="${HOST_IP%.*}.0/24"
echo "Host network: $HOST_NETWORK"
iptables -A INPUT -s "$HOST_NETWORK" -j ACCEPT
iptables -A OUTPUT -d "$HOST_NETWORK" -j ACCEPT

# 6. Default deny, allow established + allowlisted.
iptables -P INPUT DROP
iptables -P FORWARD DROP
iptables -P OUTPUT DROP
iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -m set --match-set allowed-domains dst -j ACCEPT
iptables -A OUTPUT -j REJECT --reject-with icmp-admin-prohibited

# IPv6: allow loopback only, so v6 can't bypass the v4 allowlist.
if command -v ip6tables >/dev/null && ip6tables -L >/dev/null 2>&1; then
    ip6tables -F
    ip6tables -A INPUT -i lo -j ACCEPT
    ip6tables -A OUTPUT -o lo -j ACCEPT
    ip6tables -P INPUT DROP
    ip6tables -P FORWARD DROP
    ip6tables -P OUTPUT DROP
fi

# 7. Verify.
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
