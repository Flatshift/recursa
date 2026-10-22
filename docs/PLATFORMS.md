# Platforms

What each build of Recursa can do. Generated from `crates/engine/src/caps.rs` by
`cargo test -p recursa-engine caps`; edit that file, not this one.

| Feature | macOS | Windows | iOS | Browser (CLI) |
|---|---|---|---|---|
| Export runs as CSV or JSON | yes | yes | no | yes |
| Settings in its own window | yes | yes | no | no |
| Native menus | yes | yes | no | no |
| App menu bar | yes | yes | no | no |
| Open at login, monitor from the menu bar or tray | yes | yes | no | no |
| Trace the route to a resolver | yes | yes | no | yes |

Trace route needs Administrator on Windows and CAP_NET_RAW or root on Linux; macOS runs it as the user.
