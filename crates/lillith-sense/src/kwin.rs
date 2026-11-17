//! KWin bridge receiver → `window.opened.<category>`, `window.closed`,
//! `window.minimized`, `window.private`.
//!
//! `kwin/lillith-bridge` (a KWin 6 script) calls `callDBus` on
//! [`BUS_NAME`] / [`OBJECT_PATH`] for window added/removed/activated/
//! minimized/caption/geometry and a 10 Hz cursor position (stored on the
//! shared [`Desktop`] snapshot for Follow and proximity wake), plus `Stack`
//! (the z-ordered visible normal windows) and `Area` (screen + usable work
//! area) snapshots for gravity/perching, which land in
//! [`lillith_core::desktop::shared`]. [`serve`] owns
//! that name on the session bus, replies to every call, and turns them into
//! rule events through the pure [`Bridge`] state machine. Nothing is
//! emitted unless the script is installed and enabled (M5 ships/verifies
//! it on Plasma 6); on X11 without the script there is no window source yet.

use crate::fullscreen::FullscreenTracker;
use crate::Sink;
use futures_util::StreamExt;
use lillith_core::desktop::{self, Desktop};
use lillith_core::Event;
use std::collections::HashSet;

/// D-Bus name owned by Lillith for the KWin script to call.
pub const BUS_NAME: &str = "org.lillith.Bridge";
/// Object path served.
pub const OBJECT_PATH: &str = "/org/lillith/Bridge";
/// Interface the script calls.
pub const INTERFACE: &str = "org.lillith.Bridge";

/// One call from the script (`describe(w)` = id, resourceClass, caption, x, y, w, h).
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeCall {
    Added {
        id: String,
        class: String,
        caption: String,
    },
    Removed {
        id: String,
        class: String,
    },
    Activated {
        id: String,
        class: String,
    },
    Minimized {
        id: String,
        class: String,
    },
    Caption {
        id: String,
        caption: String,
    },
    /// The active window's fullscreen state (`"1"` / `"0"` in the third
    /// string; sent on activation and on `fullScreenChanged`).
    Fullscreen {
        id: String,
        class: String,
        on: bool,
    },
    Geometry,
    /// Global pointer, logical px, +y down (`workspace.cursorPos` at 10 Hz).
    Cursor {
        x: i32,
        y: i32,
    },
    /// Window stack snapshot (see [`lillith_core::desktop::parse_stack`]).
    Stack(String),
    /// Screen + work area (see [`lillith_core::desktop::parse_area`]).
    Area(String),
}

impl BridgeCall {
    /// Builds a call from the method name and the three leading strings.
    pub fn from_parts(member: &str, id: String, class: String, caption: String) -> Option<Self> {
        Some(match member {
            "WindowAdded" => Self::Added { id, class, caption },
            "WindowRemoved" => Self::Removed { id, class },
            "WindowActivated" => Self::Activated { id, class },
            "WindowMinimized" => Self::Minimized { id, class },
            "WindowCaption" => Self::Caption { id, caption },
            "WindowFullscreen" => Self::Fullscreen {
                id,
                class,
                on: caption == "1",
            },
            "WindowGeometry" => Self::Geometry,
            "Cursor" => Self::Cursor {
                // Optional: tests may pass the pair as the first two strings.
                x: id.parse().unwrap_or(0),
                y: class.parse().unwrap_or(0),
            },
            "Stack" => Self::Stack(id),
            "Area" => Self::Area(id),
            _ => return None,
        })
    }
}

/// Pure window-event mapper.
#[derive(Debug, Default, Clone)]
pub struct Bridge {
    /// Windows already reported as private (one bubble per window).
    private_seen: HashSet<String>,
    /// Currently active window id.
    pub active: Option<String>,
    /// Window stack + work area for gravity (published by [`serve`]).
    pub desktop: Desktop,
    /// The focused fullscreen window (Ko busy).
    pub fullscreen: FullscreenTracker,
}

/// Window title markers of private/incognito browsing.
pub fn is_private_caption(caption: &str) -> bool {
    let c = caption.to_ascii_lowercase();
    [
        "private browsing",
        "incognito",
        "inprivate",
        "private window",
    ]
    .iter()
    .any(|k| c.contains(k))
}

fn ignored_class(class: &str) -> bool {
    let c = class.to_ascii_lowercase();
    c.is_empty()
        || ["lillith", "plasmashell", "krunner", "org.kde.plasmashell"]
            .iter()
            .any(|k| c == *k)
}

impl Bridge {
    /// Maps one call to events.
    pub fn handle(&mut self, call: BridgeCall) -> Vec<Event> {
        let mut out = Vec::new();
        match call {
            BridgeCall::Added { id, class, caption } => {
                if ignored_class(&class) {
                    return out;
                }
                out.push(Event::with_detail(
                    format!("window.opened.{}", categorize(&class)),
                    if caption.is_empty() {
                        class.clone()
                    } else {
                        caption.clone()
                    },
                ));
                if is_private_caption(&caption) && self.private_seen.insert(id) {
                    out.push(Event::new("window.private"));
                }
            }
            BridgeCall::Removed { id, class } => {
                self.private_seen.remove(&id);
                out.extend(self.fullscreen.gone(&id));
                if self.active.as_deref() == Some(id.as_str()) {
                    self.active = None;
                }
                if !ignored_class(&class) {
                    out.push(Event::with_detail("window.closed", class));
                }
            }
            BridgeCall::Minimized { id, class } => {
                out.extend(self.fullscreen.gone(&id));
                if !ignored_class(&class) {
                    out.push(Event::with_detail("window.minimized", class));
                }
            }
            BridgeCall::Caption { id, caption } => {
                if is_private_caption(&caption) && self.private_seen.insert(id) {
                    out.push(Event::new("window.private"));
                }
            }
            BridgeCall::Activated { id, class } => {
                // TODO(M5): focus.long_work timer and window-follow /
                // perch-on-the-focused-title-bar targets. Follow itself
                // walks the bottom edge toward the cursor's x.
                // `window.focused` carries only the window class (never the
                // caption) so memory can learn app habits; private windows
                // and shell surfaces never emit it.
                if !id.is_empty() && !ignored_class(&class) && !self.private_seen.contains(&id) {
                    out.push(Event::with_detail("window.focused", class));
                }
                self.active = (!id.is_empty()).then_some(id);
            }
            BridgeCall::Fullscreen { id, class, on } => {
                // Only the class (never the caption); shell surfaces and
                // her own window are never "Ko is busy".
                let on = on && !ignored_class(&class);
                out.extend(self.fullscreen.focused(&id, &class, on));
            }
            BridgeCall::Stack(s) => self.desktop.set_windows(desktop::parse_stack(&s)),
            BridgeCall::Area(s) => match desktop::parse_area(&s) {
                Some((screen, work)) => self.desktop.set_area(screen, work),
                None => tracing::debug!(area = %s, "KWin bridge: bad Area"),
            },
            BridgeCall::Geometry => {}
            BridgeCall::Cursor { x, y } => self.desktop.set_cursor(x, y),
        }
        out
    }
}

/// Reads the leading (id, class, caption) strings whatever numeric type
/// KWin's `callDBus` picked for the geometry (int or double).
fn parse_strings(body: &zbus::message::Body) -> (String, String, String) {
    type I = (String, String, String, i32, i32, i32, i32);
    type D = (String, String, String, f64, f64, f64, f64);
    type L = (String, String, String, i64, i64, i64, i64);
    if let Ok(v) = body.deserialize::<I>() {
        return (v.0, v.1, v.2);
    }
    if let Ok(v) = body.deserialize::<D>() {
        return (v.0, v.1, v.2);
    }
    if let Ok(v) = body.deserialize::<L>() {
        return (v.0, v.1, v.2);
    }
    if let Ok(v) = body.deserialize::<(String, String, String)>() {
        return v;
    }
    Default::default()
}

/// Reads a `Cursor(x, y)` pair, whichever numeric type `callDBus` picked.
fn parse_cursor(body: &zbus::message::Body) -> Option<(i32, i32)> {
    if let Ok((x, y)) = body.deserialize::<(i32, i32)>() {
        return Some((x, y));
    }
    if let Ok((x, y)) = body.deserialize::<(f64, f64)>() {
        return Some((x.round() as i32, y.round() as i32));
    }
    if let Ok((x, y)) = body.deserialize::<(i64, i64)>() {
        let clamp = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        return Some((clamp(x), clamp(y)));
    }
    None
}

/// Coerces a synthetic KWin `Cursor` pair (int, float or long) to pixels.
/// Used by tests; [`serve`] uses [`parse_cursor`] on the real D-Bus body.
pub fn cursor_xy_from_ints(x: i32, y: i32) -> (i32, i32) {
    (x, y)
}
/// Same as [`cursor_xy_from_ints`] for a float pair (KWin may send doubles).
pub fn cursor_xy_from_floats(x: f64, y: f64) -> (i32, i32) {
    (x.round() as i32, y.round() as i32)
}
/// Same as [`cursor_xy_from_ints`] for a 64-bit int pair.
pub fn cursor_xy_from_longs(x: i64, y: i64) -> (i32, i32) {
    let clamp = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    (clamp(x), clamp(y))
}

/// Owns [`BUS_NAME`] on the session bus and serves the script until the
/// connection closes.
pub async fn serve(sink: Sink) -> zbus::Result<()> {
    let conn = zbus::connection::Builder::session()?.build().await?;
    conn.request_name(BUS_NAME).await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::MethodCall)
        .interface(INTERFACE)?
        .path(OBJECT_PATH)?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, Some(256)).await?;
    tracing::info!("KWin bridge: serving {BUS_NAME} (enable the lillith-bridge KWin script)");
    let mut bridge = Bridge::default();
    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };
        let hdr = msg.header();
        let member = hdr.member().map(|m| m.to_string()).unwrap_or_default();
        let _ = conn.reply(&hdr, &()).await;
        let call = match member.as_str() {
            "Cursor" => match parse_cursor(&msg.body()) {
                Some((x, y)) => Some(BridgeCall::Cursor { x, y }),
                None => continue,
            },
            // Snapshots: one string argument.
            "Stack" | "Area" => match msg.body().deserialize::<(String,)>() {
                Ok((s,)) => BridgeCall::from_parts(&member, s, String::new(), String::new()),
                Err(_) => continue,
            },
            _ => {
                let (id, class, caption) = parse_strings(&msg.body());
                BridgeCall::from_parts(&member, id, class, caption)
            }
        };
        let snapshot = matches!(member.as_str(), "Stack" | "Area");
        let is_cursor = matches!(call, Some(BridgeCall::Cursor { .. }));
        if let Some(call) = call {
            for ev in bridge.handle(call) {
                sink(ev);
            }
        }
        if is_cursor {
            if let Ok(mut d) = desktop::shared().lock() {
                d.cursor = bridge.desktop.cursor;
                d.cursor_at = bridge.desktop.cursor_at;
            }
        }
        if snapshot {
            if !bridge.desktop.has_bridge {
                continue;
            }
            if let Ok(mut d) = desktop::shared().lock() {
                if d.generation == 0 {
                    tracing::info!("KWin bridge: window stack received (perching on windows on)");
                }
                let g = d.generation.wrapping_add(1);
                *d = bridge.desktop.clone();
                d.generation = g;
            }
        }
    }
    Ok(())
}

/// Window categories (copied from `aether_gesture_map.py` app lists). TODO: full lists.
pub fn categorize(resource_class: &str) -> &'static str {
    let c = resource_class.to_ascii_lowercase();
    if ["dolphin", "nautilus", "thunar", "nemo"]
        .iter()
        .any(|k| c.contains(k))
    {
        "folder"
    } else if [
        "firefox",
        "chromium",
        "chrome",
        "brave",
        "vivaldi",
        "librewolf",
    ]
    .iter()
    .any(|k| c.contains(k))
    {
        "browser"
    } else if [
        "konsole",
        "alacritty",
        "kitty",
        "foot",
        "wezterm",
        "terminal",
    ]
    .iter()
    .any(|k| c.contains(k))
    {
        "terminal"
    } else if ["vlc", "mpv", "spotify", "elisa", "haruna"]
        .iter()
        .any(|k| c.contains(k))
    {
        "media"
    } else {
        "app"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(id: &str, class: &str, caption: &str) -> BridgeCall {
        BridgeCall::Added {
            id: id.into(),
            class: class.into(),
            caption: caption.into(),
        }
    }

    #[test]
    fn bridge_maps_window_events() {
        let mut b = Bridge::default();
        assert_eq!(
            b.handle(added("1", "firefox", "Rust docs — Mozilla Firefox")),
            [Event::with_detail(
                "window.opened.browser",
                "Rust docs — Mozilla Firefox"
            )]
        );
        let ev = b.handle(added("2", "firefox", "Mozilla Firefox Private Browsing"));
        let names: Vec<&str> = ev.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["window.opened.browser", "window.private"]);
        // Caption change on an already-private window: no repeat.
        assert!(b
            .handle(BridgeCall::Caption {
                id: "2".into(),
                caption: "x — Private Browsing".into()
            })
            .is_empty());
        // Window going private later fires once.
        let ev = b.handle(BridgeCall::Caption {
            id: "1".into(),
            caption: "New Incognito tab".into(),
        });
        assert_eq!(ev, [Event::new("window.private")]);
        assert_eq!(
            b.handle(BridgeCall::Minimized {
                id: "3".into(),
                class: "org.kde.konsole".into()
            }),
            [Event::with_detail("window.minimized", "org.kde.konsole")]
        );
        // Window 1 went private above: focusing it reports nothing.
        assert!(b
            .handle(BridgeCall::Activated {
                id: "1".into(),
                class: "firefox".into(),
            })
            .is_empty());
        assert_eq!(b.active.as_deref(), Some("1"));
        assert_eq!(
            b.handle(BridgeCall::Removed {
                id: "1".into(),
                class: "firefox".into()
            }),
            [Event::with_detail("window.closed", "firefox")]
        );
        assert_eq!(b.active, None);
        // Her own window and shell surfaces are ignored.
        assert!(b.handle(added("9", "lillith", "Lillith")).is_empty());
        assert!(b.handle(added("9", "plasmashell", "")).is_empty());
        assert!(b.handle(BridgeCall::Cursor { x: 0, y: 0 }).is_empty());
    }

    #[test]
    fn focus_never_reports_private_or_shell_windows() {
        let mut b = Bridge::default();
        b.handle(added("7", "firefox", "Mozilla Firefox Private Browsing"));
        assert!(b
            .handle(BridgeCall::Activated {
                id: "7".into(),
                class: "firefox".into(),
            })
            .is_empty());
        assert_eq!(b.active.as_deref(), Some("7"));
        assert!(b
            .handle(BridgeCall::Activated {
                id: "8".into(),
                class: "plasmashell".into(),
            })
            .is_empty());
        let ev = b.handle(BridgeCall::Activated {
            id: "9".into(),
            class: "org.kde.konsole".into(),
        });
        assert_eq!(
            ev,
            [Event::with_detail("window.focused", "org.kde.konsole")]
        );
    }

    #[test]
    fn stack_and_area_fill_the_desktop_model() {
        let mut b = Bridge::default();
        assert!(b
            .handle(BridgeCall::Stack(
                "7\tdolphin\t0\t300\t800\t500\n8\tlillith\t0\t0\t9\t9".into()
            ))
            .is_empty());
        assert!(b.desktop.has_bridge);
        assert_eq!(b.desktop.windows.len(), 1, "her own window is dropped");
        assert_eq!(b.desktop.windows[0].rect.y, 300);
        b.handle(BridgeCall::Area("0 0 1920 1080 0 0 1920 1032".into()));
        assert_eq!(b.desktop.work_area.map(|r| r.bottom()), Some(1032));
        // Garbage keeps the last good area.
        b.handle(BridgeCall::Area("nope".into()));
        assert_eq!(b.desktop.work_area.map(|r| r.bottom()), Some(1032));
        assert_eq!(
            BridgeCall::from_parts("Stack", "x".into(), "".into(), "".into()),
            Some(BridgeCall::Stack("x".into()))
        );
    }

    #[test]
    fn fullscreen_focus_is_busy_until_it_ends() {
        use crate::fullscreen::{FULLSCREEN_EVENT, FULLSCREEN_LEFT_EVENT};
        let mut b = Bridge::default();
        let fs = |id: &str, class: &str, on: &str| {
            BridgeCall::from_parts("WindowFullscreen", id.into(), class.into(), on.into()).unwrap()
        };
        assert!(b.handle(fs("1", "konsole", "0")).is_empty());
        assert_eq!(
            b.handle(fs("2", "mpv", "1")),
            [Event::with_detail(FULLSCREEN_EVENT, "mpv")]
        );
        assert!(b.handle(fs("2", "mpv", "1")).is_empty(), "once");
        // Focus back on a normal window.
        assert_eq!(
            b.handle(fs("1", "konsole", "0")),
            [Event::new(FULLSCREEN_LEFT_EVENT)]
        );
        // Closing / minimizing the fullscreen window ends it too.
        b.handle(fs("3", "steam_app_9", "1"));
        let ev = b.handle(BridgeCall::Removed {
            id: "3".into(),
            class: "steam_app_9".into(),
        });
        assert_eq!(ev[0], Event::new(FULLSCREEN_LEFT_EVENT));
        b.handle(fs("4", "vlc", "1"));
        let ev = b.handle(BridgeCall::Minimized {
            id: "4".into(),
            class: "vlc".into(),
        });
        assert_eq!(ev[0], Event::new(FULLSCREEN_LEFT_EVENT));
        // Her own / shell surfaces never count.
        assert!(b.handle(fs("5", "plasmashell", "1")).is_empty());
        assert!(b.handle(fs("6", "lillith", "1")).is_empty());
    }

    #[test]
    fn from_parts() {
        assert_eq!(
            BridgeCall::from_parts("Nope", "".into(), "".into(), "".into()),
            None
        );
        assert_eq!(
            BridgeCall::from_parts("Cursor", "".into(), "".into(), "".into()),
            Some(BridgeCall::Cursor { x: 0, y: 0 })
        );
        assert_eq!(
            BridgeCall::from_parts("Cursor", "100".into(), "200".into(), "".into()),
            Some(BridgeCall::Cursor { x: 100, y: 200 })
        );
    }

    #[test]
    fn cursor_message_parses_int_float_or_long_into_desktop() {
        let mut b = Bridge::default();
        assert_eq!(b.desktop.cursor, None);
        // int
        let (x, y) = cursor_xy_from_ints(320, 1080);
        assert!(b.handle(BridgeCall::Cursor { x, y }).is_empty());
        assert_eq!(b.desktop.cursor, Some((320, 1080)));
        assert!(b.desktop.cursor_at.is_some());
        assert!(!b.desktop.has_bridge);
        assert_eq!(b.desktop.generation, 0);
        // float (KWin may send doubles); half-up via round
        let (x, y) = cursor_xy_from_floats(10.4, 11.6);
        assert_eq!((x, y), (10, 12));
        b.handle(BridgeCall::Cursor { x, y });
        assert_eq!(b.desktop.cursor, Some((10, 12)));
        // long
        let (x, y) = cursor_xy_from_longs(640i64, -20i64);
        assert_eq!((x, y), (640, -20));
        b.handle(BridgeCall::Cursor { x, y });
        assert_eq!(b.desktop.cursor, Some((640, -20)));
        // A later Stack snapshot keeps the last cursor.
        b.handle(BridgeCall::Stack("7\tdolphin\t0\t300\t800\t500".into()));
        assert_eq!(b.desktop.cursor, Some((640, -20)));
        assert!(b.desktop.has_bridge);
    }

    #[test]
    fn window_rules_exist() {
        let rules = lillith_core::rules::RuleSet::bundled();
        for e in ["window.closed", "window.minimized", "window.private"] {
            assert!(rules.find(e).is_some(), "{e}");
        }
    }

    #[test]
    fn categories_have_rules() {
        let rules = lillith_core::rules::RuleSet::bundled();
        for class in ["dolphin", "firefox", "konsole", "mpv", "kate"] {
            let ev = format!("window.opened.{}", categorize(class));
            assert!(rules.find(&ev).is_some(), "no rule for {ev}");
        }
    }
}
