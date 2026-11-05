# pila's features and their tests

Every feature and invariant pila has (`docs/ARCHITECTURE.md`), and the tests that cover it: integration tests as
file::test (in tests/FILE.rs, on the headless backend), unit tests as
src/FILE.rs::test. `tests/features_inventory.rs` checks that every test
named here exists and that every integration test is named here, so this
file can't fall behind. A feature without a test says why: it runs only
on bare metal (`udev.rs`: `scripts/check` on a TTY runs the DRM/KMS
backend through real programs and checks frame times; what it doesn't
exercise, the user checks by hand) or only eyes can judge it. Known bugs are tests marked `#[ignore]`
(`cargo test -- --ignored` runs them); they're listed at the end.

## Harness

| Feature | Tests |
|---|---|
| Headless backend, control socket, state dump | `smoke::headless_pila_starts_with_one_output`, `smoke::a_window_maps_focused_and_is_drawn`, `smoke::bindings_run_from_injected_keys`, `smoke::quit_binding_exits`, `src/headless.rs::output_specs_parse` |

## Command line and configuration

| Feature | Tests |
|---|---|
| `--version`, `--help`, bad arguments | `cli::version_and_help`, `cli::bad_arguments_exit_2` |
| `--check-config` (file and keymap) | `cli::check_config_accepts_a_valid_file`, `cli::check_config_refuses_an_invalid_file_and_says_why`, `cli::check_config_refuses_a_bad_keymap`, `cli::check_config_of_a_missing_file_is_fine`, `cli::the_example_config_checks`, `rules::invalid_rules_are_refused` |
| Config parsing (every section, defaults, env overrides) | `src/config.rs` unit tests (25), `bindings::pila_mod_overrides_the_config`, `bindings::pila_terminal_overrides_the_config`, `bindings::mod_d_runs_the_launcher_and_pila_launcher_overrides_it` |
| Reload: each section applied, invalid files kept out | `config_reload::a_new_binding_works_after_a_reload`, `config_reload::an_invalid_file_keeps_the_running_config`, `config_reload::a_syntax_error_keeps_the_running_config`, `config_reload::the_keyboard_layout_changes_on_reload`, `config_reload::an_invalid_keymap_keeps_the_current_one`, `config_reload::the_mod_key_changes_on_reload`, `config_reload::output_scale_position_and_transform_apply_on_reload`, `config_reload::an_output_disabled_by_the_config_moves_its_windows`, `config_reload::every_output_off_leaves_one_on`, `config_reload::a_reload_with_an_unchanged_file_keeps_windows_as_they_are`, `workspaces::fewer_workspaces_after_a_reload_bring_windows_to_the_last_one`, `rules::rules_are_resolved_again_on_reload_but_not_on_title_changes` |
| Autostart, `-c`, session environment of children | `config_reload::autostart_runs_once_not_on_reload`, `config_reload::a_command_line_command_replaces_autostart`, `bindings::spawned_commands_get_the_session_environment` |
| udev backend fails cleanly without a seat | `cli::the_udev_backend_without_a_seat_fails_cleanly` |

## Keybindings

| Feature | Tests |
|---|---|
| Every default binding | `bindings::mod_shift_q_closes_and_the_client_never_sees_q`, `bindings::mod_m_and_mod_up_maximize_and_toggle_back`, `bindings::mod_f_toggles_fullscreen_over_the_whole_output`, `bindings::mod_n_minimizes`, `bindings::mod_left_and_right_snap_to_halves_and_again_restores`, `bindings::mod_down_undoes_a_snap`, `bindings::mod_shift_arrows_move_by_40`, `bindings::mod_ctrl_arrows_resize_by_40`, `bindings::keyboard_resizes_stop_at_40`, `bindings::mod_c_centers`, `bindings::mod_return_runs_the_terminal`, `bindings::mod_d_without_a_launcher_does_nothing`, `switcher::mod_j_switches_too`, `workspaces::page_keys_step_without_wrapping`, `workspaces::mod_shift_page_keys_move_the_window`, `smoke::quit_binding_exits` |
| Exact modifiers, locks ignored, release kept from the client, unbinding, keys without Mod, Mod setting | `bindings::modifiers_must_match_exactly`, `bindings::caps_lock_and_num_lock_dont_stop_bindings`, `bindings::plain_keys_reach_the_focused_client`, `bindings::a_binding_set_to_none_is_removed`, `bindings::a_key_without_mod_can_be_bound`, `bindings::mod_can_be_alt`, `bindings::digits_past_the_workspaces_reach_the_client`, `bindings::bindings_ignore_unfocused_state_without_a_window`, `bindings::moving_or_resizing_a_filled_window_does_nothing` |
| Shortcuts inhibitor (Quit exempt) | `input_protocols::an_inhibitor_gets_the_bindings_keys_but_not_quit`, `input_protocols::quit_still_works_with_an_inhibitor`, `input_protocols::an_inhibitor_only_counts_while_its_surface_has_focus` |
| Ctrl+Alt+F*n* VT switch | bare metal, by hand (the user) |

## Windows

| Feature | Tests |
|---|---|
| xdg-session-management: sessions, restoring windows, saving, errors | `session_management::a_new_session_gets_an_unguessable_id`, `session_management::a_window_goes_back_to_its_place_and_size`, `session_management::restored_comes_before_the_first_configure`, `session_management::a_toplevel_just_added_isnt_told_restored`, `session_management::a_client_unmapping_before_destroying_keeps_the_size`, `session_management::a_window_mapped_again_without_restoring_is_a_new_one`, `session_management::without_asking_to_restore_a_window_opens_as_usual`, `session_management::a_window_outside_any_session_opens_as_usual`, `session_management::an_unknown_name_is_like_adding_it`, `session_management::a_taken_spot_cascades`, `session_management::a_window_partly_off_the_output_comes_back_inside`, `session_management::a_window_closed_maximized_comes_back_maximized`, `session_management::a_window_closed_fullscreen_comes_back_fullscreen`, `session_management::a_window_closed_snapped_comes_back_where_it_was_before`, `session_management::a_window_closed_on_another_workspace_comes_back_on_the_active_one`, `session_management::it_goes_back_to_its_output_wherever_the_pointer_is`, `session_management::a_filled_window_goes_back_to_its_output`, `session_management::with_its_output_gone_it_opens_as_usual_with_its_size`, `session_management::rules_win_over_the_session`, `session_management::renaming_keeps_the_state`, `session_management::removing_a_toplevel_forgets_it`, `session_management::removing_a_session_forgets_it`, `session_management::a_destroyed_session_keeps_its_state_and_stops_following`, `session_management::a_dead_clients_session_can_be_taken_up_again`, `session_management::another_client_takes_over_and_the_old_session_goes_inert`, `session_management::sessions_outlive_pila`, `session_management::sessions_are_saved_soon_and_only_for_the_user`, `session_management::a_broken_sessions_file_is_ignored`, `session_management::an_unknown_session_id_gets_a_new_session`, `session_management::a_session_in_use_by_the_same_client_is_an_error`, `session_management::an_empty_session_id_is_an_error`, `session_management::an_empty_toplevel_name_is_an_error`, `session_management::adding_a_known_name_is_an_error`, `session_management::restoring_a_name_in_use_is_an_error`, `session_management::adding_a_toplevel_twice_is_an_error`, `session_management::restoring_after_the_first_commit_is_an_error`, `session_management::renaming_to_a_name_in_use_is_an_error`, `src/session_management.rs` unit tests |
| Mapping, placement, cascade, output under the pointer | `windows::a_window_opens_centered_below_its_title_bar`, `windows::windows_on_the_same_spot_cascade`, `windows::a_window_opens_on_the_output_under_the_pointer`, `windows::a_window_larger_than_the_output_starts_at_its_top_left`, `src/state.rs::centered_location_centers_window_and_title_bar` |
| Focus, raise, click to focus, stacking | `windows::a_new_window_takes_focus_and_goes_on_top`, `windows::clicking_a_window_focuses_and_raises_it_and_the_click_reaches_it`, `windows::clicking_the_background_clears_focus`, `windows::the_pointer_enters_the_window_under_it_at_surface_coordinates`, `windows::closing_the_focused_window_focuses_the_one_below` |
| Lifecycle: unmap, dead clients, titles | `windows::a_window_unmapped_by_its_client_leaves_and_maps_again`, `windows::an_unmapped_window_leaves_taskbars_and_comes_back_as_a_new_one`, `windows::a_dead_client_takes_its_windows_along`, `windows::title_and_app_id_changes_show_in_the_state` |
| Decorations: negotiation, title bar buttons, drag | `windows::a_client_side_decorated_window_has_no_bar`, `windows::a_window_asking_for_client_side_gets_it`, `windows::server_side_decorations_are_confirmed`, `windows::the_title_bar_close_button_asks_the_window_to_close`, `windows::the_title_bar_maximize_button_toggles_maximized`, `windows::the_title_bar_minimize_button_minimizes`, `windows::the_title_bar_drags_the_window`, `src/decoration.rs` unit tests |
| Maximize / fullscreen, restore geometry, refits | `fill_states::a_client_can_maximize_and_unmaximize_itself`, `fill_states::a_client_can_go_fullscreen_and_back`, `fill_states::fullscreen_over_maximized_comes_back_maximized`, `fill_states::a_window_that_opens_maximized_is_restored_to_two_thirds_centered`, `fill_states::a_window_that_opens_fullscreen_fills_the_output`, `fill_states::fullscreen_on_a_named_output`, `fill_states::maximized_windows_refit_when_the_output_changes_mode`, `fill_states::a_maximized_window_keeps_its_restore_geometry_across_workspaces`, `fill_states::maximizing_a_snapped_window_restores_to_before_the_snap`, `fill_states::toplevel_bounds_are_the_usable_area` |
| Snapping and keyboard moves | `bindings::mod_left_and_right_snap_to_halves_and_again_restores`, `grabs::dragging_to_the_left_edge_snaps_with_a_preview`, `grabs::dragging_to_the_top_edge_maximizes`, `grabs::moving_away_from_the_edge_drops_the_preview`, `grabs::dragging_a_snapped_window_restores_its_size`, `grabs::resizing_a_snapped_window_forgets_the_snap`, `src/snap.rs` unit tests |
| Pointer grabs: Mod+drag, border, client requests, top/left resize | `grabs::mod_left_drag_moves_and_the_client_never_sees_the_click`, `grabs::mod_right_drag_resizes_from_the_nearest_corner`, `grabs::resizing_from_the_top_left_moves_the_window_once_it_commits`, `grabs::a_resize_respects_the_minimum_size`, `grabs::the_invisible_border_resizes`, `grabs::the_border_reaches_around_the_title_bar`, `grabs::a_client_side_decorated_window_has_no_border`, `grabs::a_client_can_start_a_move_while_its_button_is_held`, `grabs::a_client_cant_start_a_move_without_a_held_button`, `grabs::a_client_can_start_a_resize`, `grabs::dragging_a_maximized_window_restores_it_under_the_pointer` |
| Window switching (Mod+Tab) | `switcher::mod_tab_switches_to_the_previous_window_and_ends_on_release`, `switcher::holding_mod_and_tapping_tab_goes_further_back`, `switcher::mod_shift_tab_goes_the_other_way`, `switcher::a_click_ends_the_switch`, `switcher::with_one_window_mod_tab_does_nothing`, `src/switcher.rs::steps_wrap_around_both_ways` |
| Minimize | `switcher::minimizing_focuses_the_most_recent_window_left`, `switcher::mod_tab_shows_a_minimized_window_while_selected_and_keeps_it_when_ending_there`, `switcher::a_lone_minimized_window_comes_back_with_mod_tab`, `switcher::a_client_can_minimize_itself`, `switcher::a_minimized_window_whose_client_dies_is_forgotten` |
| Workspaces | `workspaces::switching_parks_the_windows_and_brings_them_back`, `workspaces::a_window_opens_on_the_active_workspace`, `workspaces::mod_shift_digit_moves_the_focused_window_and_stays`, `workspaces::the_most_recent_window_of_a_workspace_gets_focus_back`, `workspaces::a_window_being_dragged_comes_along`, `workspaces::a_dragged_window_cant_be_sent_away`, `workspaces::a_minimized_window_keeps_its_workspace`, `src/workspaces.rs::targets_resolve_without_wrapping` |
| Window and layer rules | `rules::a_rule_sizes_and_places_a_window`, `rules::a_rule_opens_a_window_maximized_or_fullscreen`, `rules::a_rule_opens_a_window_on_another_workspace_unfocused`, `rules::title_bar_false_draws_no_bar_on_a_server_side_window`, `rules::opacity_blends_the_window_with_whats_behind`, `rules::corner_radius_cuts_the_corners`, `rules::the_global_corner_radius_applies_and_a_rule_overrides_it`, `rules::all_matchers_must_match_and_wildcards_work`, `rules::a_rule_without_matchers_matches_everything`, `rules::later_rules_win_field_by_field`, `rules::a_bar_hidden_by_reload_moves_a_maximized_window_up`, `rules::an_animation_rule_turns_one_kind_off`, `layers::a_layer_rule_matches_by_namespace`, `src/rules.rs` unit tests |

## Shell surfaces

| Feature | Tests |
|---|---|
| Layer shell: placement, zones, stacking, keyboard, outputs | `layers::a_bar_is_stretched_and_takes_its_exclusive_zone`, `layers::windows_open_and_maximize_inside_the_usable_area`, `layers::maximized_windows_refit_when_the_zone_changes_or_goes`, `layers::layers_stack_around_windows`, `layers::an_exclusive_keyboard_layer_keeps_the_keyboard`, `layers::an_on_demand_layer_gets_the_keyboard_when_clicked`, `layers::clicking_a_background_layer_clears_focus`, `layers::a_layer_goes_on_the_output_it_asks_for`, `layers::margins_and_anchors_place_a_layer`, `layers::layer_surfaces_on_a_removed_output_are_closed` |
| Popups: positioners, constraints, grabs, nesting | `popups::a_popup_goes_where_its_positioner_says`, `popups::a_popup_slides_back_onto_the_output_when_allowed`, `popups::a_popup_flips_when_allowed_and_that_fits`, `popups::a_popup_doesnt_flip_when_flipped_wouldnt_fit_either`, `popups::without_adjustments_a_popup_stays_where_asked`, `popups::a_grabbing_popup_takes_the_keyboard_and_a_click_outside_dismisses_it`, `popups::a_new_window_dismisses_a_grabbing_popup`, `popups::mod_tab_dismisses_a_grabbing_popup`, `popups::a_popup_of_a_popup_is_placed_against_its_parent`, `popups::a_layer_surface_can_have_popups`, `popups::a_popup_closes_with_its_window` |
| Session lock | `lock::locking_shows_only_the_lock_surface_and_gives_it_the_keyboard`, `lock::bindings_and_workspace_switches_dont_run_while_locked`, `lock::clicks_dont_reach_windows_while_locked`, `lock::new_windows_dont_take_focus_while_locked`, `lock::unlocking_gives_the_screen_and_keyboard_back`, `lock::a_dead_locker_leaves_the_session_locked_and_another_can_take_over`, `lock::a_second_lock_while_locked_is_refused`, `lock::destroying_the_lock_without_unlocking_keeps_it_locked`, `lock::an_output_without_a_lock_surface_shows_black`, `lock::popup_grabs_and_moves_are_refused_while_locked`, `outputs::a_lock_is_confirmed_with_an_output_turned_off` |

## Input protocols

| Feature | Tests |
|---|---|
| Idle notify and inhibit | `input_protocols::idle_notifications_fire_after_the_timeout_and_resume_on_input`, `input_protocols::an_idle_inhibitor_on_a_shown_window_stops_idling`, `input_protocols::an_inhibitor_on_a_minimized_window_doesnt_count` |
| Pointer constraints, relative motion | `input_protocols::a_locked_pointer_stays_put_and_gets_relative_motion`, `input_protocols::a_lock_only_activates_on_the_focused_window_under_the_pointer`, `input_protocols::a_confined_pointer_stays_inside_the_surface`, `input_protocols::switching_to_an_empty_workspace_releases_a_locked_pointer`, `input_protocols::a_oneshot_lock_ends_when_released` |
| Virtual keyboard | `input_protocols::a_virtual_keyboard_types_into_the_focused_window_past_the_bindings` |
| xdg-activation | `input_protocols::a_focused_client_can_activate_another_window`, `input_protocols::an_unfocused_client_cant_steal_focus`, `input_protocols::activation_brings_a_window_from_another_workspace_or_minimized` |
| Gestures | `gestures::a_three_finger_swipe_left_switches_windows_and_stays_with_pila`, `gestures::a_long_swipe_steps_through_more_windows`, `gestures::three_fingers_up_maximize_and_down_restore`, `gestures::four_fingers_change_workspace`, `gestures::a_short_swipe_does_nothing`, `gestures::unbound_swipes_pinches_and_holds_reach_the_client`, `gestures::a_shortcuts_inhibitor_gets_the_swipe`, `gestures::gestures_do_nothing_while_locked`, `gestures::a_gesture_config_rebinds_swipes`, `src/gestures.rs` unit tests |
| libinput device settings (`[input]`), keyboard LEDs | bare metal, by hand (the user) |

## Surfaces and clients

| Feature | Tests |
|---|---|
| Globals offered; hidden from sandboxed clients | `surface_protocols::pila_offers_exactly_these_globals`, `surface_protocols::sandboxed_clients_dont_see_privileged_globals` |
| System bell | `surface_protocols::the_bell_flashes_the_window_that_rang`, `surface_protocols::a_bell_without_a_surface_flashes_the_focused_window`, `surface_protocols::a_subsurfaces_bell_flashes_its_window`, `render::the_bell_flash_lightens_the_window` |
| Selections: clipboard, primary, data-control | `surface_protocols::the_clipboard_goes_from_one_client_to_another`, `surface_protocols::an_unfocused_client_gets_no_offer`, `surface_protocols::data_control_reads_and_sets_the_clipboard`, `surface_protocols::the_primary_selection_goes_between_clients` |
| Output scale, fractional scale, viewporter | `surface_protocols::a_fractional_output_scale_reaches_the_window`, `surface_protocols::a_window_moved_to_another_output_gets_its_scale`, `surface_protocols::a_viewport_sets_the_window_size_and_single_pixel_buffers_fill_it` |
| Cursors | `surface_protocols::a_client_sets_named_hidden_and_surface_cursors`, `surface_protocols::a_stale_serial_cant_set_the_cursor`, `src/cursor.rs` unit test; xcursor themes on bare metal |
| Subsurfaces | `surface_protocols::a_subsurface_is_drawn_and_gets_the_pointer` |
| Frame callbacks and presentation feedback | `surface_protocols::frame_callbacks_and_presentation_feedback_follow_a_commit`, `surface_protocols::a_covered_window_gets_frame_callbacks_about_once_a_second` |

## Outputs

| Feature | Tests |
|---|---|
| Plugging, unplugging, where windows and the pointer go | `outputs::a_new_output_goes_right_of_the_others_and_clients_see_it`, `outputs::unplugging_an_output_moves_its_windows_and_the_pointer`, `outputs::a_maximized_window_on_an_unplugged_output_refits_and_restores_onto_another`, `outputs::a_window_on_another_workspace_comes_back_onto_a_remaining_output`, `outputs::a_window_belongs_to_the_output_holding_most_of_it`, `outputs::unplugging_the_last_output_and_plugging_one_back_works` |
| wlr-output-management | `outputs::heads_describe_the_outputs`, `outputs::applying_a_configuration_changes_the_outputs`, `outputs::testing_a_configuration_changes_nothing`, `outputs::bad_configurations_fail`, `outputs::a_configuration_for_an_old_serial_is_cancelled`, `outputs::disabling_a_head_removes_the_output_but_keeps_the_head`, `outputs::editing_an_outputs_config_section_drops_its_override` |
| Output power and gamma (protocol side) | `outputs::an_output_can_be_turned_off_and_on`, `outputs::gamma_ramps_are_set_and_reset`, `outputs::a_second_gamma_control_or_a_short_ramp_fails`, `src/gamma_control.rs` unit tests |
| `disable-internal-display` | `src/udev.rs::the_built_in_screen_goes_off_only_for_an_external_one_that_is_on`, `src/udev.rs::built_in_screens_are_recognized_by_connector`; the rest on bare metal |
| DRM/KMS: modes, CRTCs, vblank pacing, VRR, gamma ioctl, power, VT switching | bare metal: `scripts/check` (modes, CRTCs, vblank pacing), the rest by hand (the user); `src/udev.rs::linear_ramps_are_recognized` |

## Taskbars and capture

| Feature | Tests |
|---|---|
| ext-foreign-toplevel-list | `taskbar::the_list_announces_windows_their_changes_and_their_end`, `taskbar::windows_opened_later_are_announced` |
| wlr-foreign-toplevel-management | `taskbar::handles_report_titles_states_and_outputs`, `taskbar::activate_focuses_a_window`, `taskbar::minimize_and_unminimize`, `taskbar::maximize_fullscreen_and_close`, `taskbar::activating_a_window_elsewhere_switches_workspace`, `taskbar::requests_are_ignored_while_locked` |
| ext-workspace | `taskbar::workspaces_are_listed_with_the_active_one`, `taskbar::a_client_activates_a_workspace_on_commit`, `taskbar::a_binding_switch_is_reported`, `taskbar::a_new_workspace_count_adds_and_removes_workspaces` |
| ext-image-copy-capture | `capture::an_output_capture_has_the_screens_pixels`, `capture::a_window_capture_has_only_the_window`, `capture::a_later_frame_waits_for_damage`, `capture::a_buffer_of_the_wrong_size_fails`, `capture::closing_the_window_stops_its_session`, `capture::closing_the_window_stops_an_idle_session`, `capture::unplugging_the_output_stops_its_session`, `capture::a_scaled_output_is_captured_at_its_pixel_size`, `capture::window_captures_wait_while_locked`, `capture::an_output_capture_while_locked_shows_the_lock` |
| wlr-screencopy | `capture::screencopy_copies_an_output`, `capture::screencopy_of_a_region`, `capture::screencopy_with_damage_waits_for_a_change`, `capture::screencopy_of_an_unplugged_output_fails` |
| Capture into dma-bufs | bare metal and nested (`scripts/check`: grim) |

## X11

| Feature | Tests |
|---|---|
| Xwayland windows | `x11::an_x11_window_is_managed_by_its_wm_class`, `x11::keys_reach_the_focused_x11_window`, `x11::mod_shift_q_asks_an_x11_window_to_close`, `x11::net_wm_state_requests_maximize_and_fullscreen`, `x11::pila_maximizing_an_x11_window_sets_its_state`, `x11::an_iconify_request_minimizes`, `x11::clicking_an_x11_window_focuses_it`, `x11::an_override_redirect_window_goes_where_x_puts_it_and_isnt_switched_to`, `x11::rules_size_and_fill_x11_windows`, `x11::an_x11_window_on_another_workspace_is_hidden_from_x_too`, `x11::a_moved_x11_window_is_told_where_it_is` |
| Selections with X11 | `x11::the_clipboard_goes_from_wayland_to_a_focused_x11_app`, `x11::the_clipboard_goes_from_x11_to_a_wayland_client`, `x11::an_unfocused_x11_app_cant_read_the_clipboard` |

## Drawing, animations, performance

| Feature | Tests |
|---|---|
| Title bars, hover, rounded corners, snap preview | `render::the_title_bar_shows_focus`, `render::a_client_side_decorated_window_has_nothing_above_it`, `render::hovering_the_close_button_highlights_it`, `render::rounded_corners_include_the_bar_and_go_square_when_maximized`, `render::the_snap_preview_is_drawn_while_dragging_to_an_edge`, `src/rounded.rs` unit tests; title text and how it all looks: the user |
| Blur | `render::a_blur_rule_blurs_whats_behind_a_layer`, `render::without_blur_the_glass_is_clear`, `render::a_client_can_ask_for_blur_behind_a_region`; how it looks: the user |
| Animations | `animations::opening_and_closing_animate_and_the_ghost_goes_away`, `animations::minimizing_restoring_maximizing_and_switching_workspaces_animate`, `animations::transient_layer_surfaces_fade_in_and_out_but_bars_dont`, `animations::a_kind_set_to_false_doesnt_animate`, `animations::enabled_false_turns_them_all_off`, `animations::speed_stretches_the_durations`, `animations::a_window_animates_through_a_texture_while_opening`, `animations::a_new_animation_starts_from_where_the_last_one_was`, `animations::locking_drops_running_animations`, `src/animation.rs` unit tests; how they look: the user |
| Redraws on demand | `animations::nothing_is_drawn_while_nothing_changes`, `animations::an_animation_is_drawn_every_refresh` |
| Frame budget, missed refreshes | `src/frame_stats.rs` unit tests; real numbers: `scripts/check` (nested and bare metal) |

## Known bugs (ignored tests)

None at the moment.
