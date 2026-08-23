## `wayclip-daemon`

[![Crates.io](https://img.shields.io/crates/v/wayclip-daemon.svg)](https://crates.io/crates/wayclip-daemon)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

The crate `wayclip-daemon` provides important processes and pipelines for intialising, managing and debugging the Wayclip daemon.
Wayclip daemon provides the functionality to record and process data.

> **Note:** `wayclip-daemon` is designed as a background service and is controlled via `wayclip-cli`.

## Libraries Required

| Package Name                 | Required? | Minimum Version | Notes                                                                         |
| ---------------------------- | --------- | --------------- | ----------------------------------------------------------------------------- |
| `gstreamer-1.0`              | Yes       | `>= 1.14`       | Required by `gstreamer`, `gstreamer-app`, `gstreamer-pbutils`                 |
| `gstreamer-plugins-base-1.0` | Yes       | `>= 1.14`       | Provides elements like `appsrc`, `videoconvert`, `videoscale`, etc...         |
| `gstreamer-plugins-good`     | Yes       | `>= 1.14`       | Provides elements like `matroskamux` & `watchdog`                             |
| `gstreamer-plugins-bad`      | Yes       | `>= 1.14`       | Provides encoders like `h264parse`, `vah264enc` (VAAPI), `nvh264enc` (NVIDIA) |
| `gstreamer-plugins-ugly`     | Yes       | `>= 1.14`       | Provides software encoder `x264enc`                                           |
| `libpipewire-0.3`            | Yes       | `>= 0.3.0`      | Required by `pipewire`                                                        |
| `libudev`                    | Yes       | `>= 199`        | Required by `gilrs`                                                           |
| `alsa-lib`                   | Yes       | `>= 1.0.27`     | Required by `rodio`                                                           |

| Runtime Dependency                      | Required?   | Notes                                                                |
| --------------------------------------- | ----------- | -------------------------------------------------------------------- |
| `xdg-desktop-portal` (backend specific) | Yes         | Required by `ashpd`'s `Screencast` portal                            |
| D-Bus                                   | Yes         | Required by `zbus`                                                   |
| `pipewire`                              | Yes         | Required by `pipewire` & checked using `$XDG_RUNTIME_DIR/pipewire-0` |
| `systemd`                               | Optional    | Required by `DaemonManager`, controls via `org.freedesktop.systemd1` |
| `hyprctl` / `swaymsg`                   | Conditional | Only for Hyprland/Sway Key-Binding & Window Discovery                |
| `gdbus`                                 | Conditional | Only for GNOME window discovery                                      |
| Discord client                          | Conditional | Only if `discord_rich_presence` is enabled in settings               |

The runtime-required `gstreamer` packages can be checked via the `wayclip-cli` by running `wayclip daemon doctor`

## Platforms supported

| Platform                  | Status    | Notes                                                                           |
| ------------------------- | --------- | ------------------------------------------------------------------------------- |
| Linux (Wayland/Hyprland)  | Supported | Key-Binding done via `hyprctl`                                                  |
| Linux (Wayland/Sway)      | Supported | Key-Binding done via `swaymsg`                                                  |
| Linux (Wayland/GNOME/KDE) | Supported | Key-Binding done via `wayclip-global-hotkey`. Window Discovery done via `gdbus` |
| Linux (Wayland/Other)     | Supported | Key-Binding not supported                                                       |
| Linux (X11/Other)         | Supported | Key-Binding not tested                                                          |
| Windows                   | Planned   | Not available                                                                   |
| MacOS                     | Unknown   | Not available                                                                   |

## Feature flags

| Feature Flag | Description                                                                           | Default |
| ------------ | ------------------------------------------------------------------------------------- | ------- |
| `linux`      | Enables the `linux` module, enabling use of packages line `zbus`, `pipewire` and more | No      |
| `windows`    | Not currently avaialable                                                              | No      |

> _Note: `linux` and `windows` feature flags are mutually exclusive._

## License

This project is licensed under the [MIT License](LICENSE.md).
