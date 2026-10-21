//! KWin bridge receiver → `window.opened.<category>`, `window.closed`,
//! `window.minimized`, `window.private`.
//!
//! `kwin/lillith-bridge` (a KWin 6 script) calls `callDBus` on
//! [`BUS_NAME`] / [`OBJECT_PATH`] for window added/removed/activated/
//! minimized/caption/geometry and a 10 Hz cursor position, plus `Stack`
//! (the z-ordered visible normal windows) and `Area` (screen + usable work
//! area) snapshots for gravity/perching, which land in
//! [`lillith_core::desktop::shared`]. [`serve`] owns
//! that name on the session bus, replies to every call, and turns them into
//! rule events through the pure [`Bridge`] state machine. Nothing is
//! emitted unless the script is installed and enabled (M5 ships/verifies
//! it on Plasma 6); on X11 without the script there is no window source yet.

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
    Geometry,
    Cursor,
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
            "WindowGeometry" => Self::Geometry,
            "Cursor" => Self::Cursor,
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
                if self.active.as_deref() == Some(id.as_str()) {
                    self.active = None;
                }
                if !ignored_class(&class) {
                    out.push(Event::with_detail("window.closed", class));
                }
            }
            BridgeCall::Minimized { class, .. } => {
                if !ignored_class(&class) {
                    out.push(Event::with_detail("window.minimized", class));
                }
            }
            BridgeCall::Caption { id, caption } => {
                if is_private_caption(&caption) && self.private_seen.insert(id) {
                    out.push(Event::new("window.private"));
                }
            }
            BridgeCall::Activated { id, .. } => {
                // TODO(M5): focus.long_work timer and Follow/perch targets.
                self.active = (!id.is_empty()).then_some(id);
            }
            BridgeCall::Stack(s) => self.desktop.set_windows(desktop::parse_stack(&s)),
            BridgeCall::Area(s) => match desktop::parse_area(&s) {
                Some((screen, work)) => self.desktop.set_area(screen, work),
                None => tracing::debug!(area = %s, "KWin bridge: bad Area"),
            },
            BridgeCall::Geometry | BridgeCall::Cursor => {}
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
        let (id, class, caption) = match member.as_str() {
            "Cursor" => Default::default(),
            // Snapshots: one string argument.
            "Stack" | "Area" => match msg.body().deserialize::<(String,)>() {
                Ok((s,)) => (s, String::new(), String::new()),
                Err(_) => continue,
            },
            _ => parse_strings(&msg.body()),
        };
        let snapshot = matches!(member.as_str(), "Stack" | "Area");
        if let Some(call) = BridgeCall::from_parts(&member, id, class, caption) {
            for ev in bridge.handle(call) {
                sink(ev);
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
        b.handle(BridgeCall::Activated {
            id: "1".into(),
            class: "firefox".into(),
        });
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
        assert!(b.handle(BridgeCall::Cursor).is_empty());
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
    fn from_parts() {
        assert_eq!(
            BridgeCall::from_parts("Nope", "".into(), "".into(), "".into()),
            None
        );
        assert_eq!(
            BridgeCall::from_parts("Cursor", "".into(), "".into(), "".into()),
            Some(BridgeCall::Cursor)
        );
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
