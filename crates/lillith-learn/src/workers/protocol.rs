//! The worker ↔ parent wire format: one JSON object per line.
//!
//! Worker → parent (stdout), exactly these shapes and no other keys:
//!
//! - `{"t":"hb"}` — heartbeat, every `heartbeat_s`;
//! - `{"t":"get","url":"https://…"}` — please fetch this (read-only GET;
//!   there is no method, header, body, cookie or form field);
//! - `{"t":"tokens","n":123}` — model tokens used since the last report;
//! - `{"t":"fact","title":"…","text":"…","url":"https://…"}` — a candidate
//!   fact from a page the parent delivered;
//! - `{"t":"done"}`.
//!
//! Anything else (an unknown type, an extra key, a write attempt such as
//! `method`/`body`/`form`/`cookie`, an install or exec request) is a
//! protocol anomaly: kill + quarantine. There is no message that installs,
//! downloads to disk, runs a program or writes memory.
//!
//! Parent → worker (stdin): `{"t":"task",…}` once, then one
//! `{"t":"page","url","title","text"}` or `{"t":"error","url","why"}` per
//! `get`.

use serde_json::Value;

/// One worker message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Heartbeat,
    Get {
        url: String,
    },
    Tokens(u32),
    Fact {
        title: String,
        text: String,
        url: String,
    },
    Done,
}

/// Keys that mean "write something / be someone" on a request.
const WRITE_KEYS: &[&str] = &[
    "method", "body", "data", "form", "post", "put", "upload", "file", "files", "cookie",
    "cookies", "headers", "header", "auth", "login", "password", "user", "username", "token",
];

fn exact_keys(o: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<(), String> {
    for k in o.keys() {
        if !keys.contains(&k.as_str()) {
            if WRITE_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                return Err(format!("write attempt: {k:?} (workers are read-only GET)"));
            }
            return Err(format!("protocol: unexpected field {k:?}"));
        }
    }
    Ok(())
}

fn string(o: &serde_json::Map<String, Value>, k: &str) -> Result<String, String> {
    o.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("protocol: {k:?} missing or not a string"))
}

/// Parses one line. `Err` is the anomaly reason.
pub fn parse(line: &str) -> Result<Msg, String> {
    let v: Value = serde_json::from_str(line.trim())
        .map_err(|_| "protocol: not one JSON object".to_string())?;
    let Some(o) = v.as_object() else {
        return Err("protocol: not one JSON object".into());
    };
    let t = o.get("t").and_then(Value::as_str).unwrap_or("");
    match t {
        "hb" => exact_keys(o, &["t"]).map(|_| Msg::Heartbeat),
        "done" => exact_keys(o, &["t"]).map(|_| Msg::Done),
        "get" => {
            exact_keys(o, &["t", "url"])?;
            Ok(Msg::Get {
                url: string(o, "url")?,
            })
        }
        "tokens" => {
            exact_keys(o, &["t", "n"])?;
            let n = o
                .get("n")
                .and_then(Value::as_u64)
                .ok_or("protocol: \"n\" missing")?;
            Ok(Msg::Tokens(u32::try_from(n).unwrap_or(u32::MAX)))
        }
        "fact" => {
            exact_keys(o, &["t", "title", "text", "url"])?;
            Ok(Msg::Fact {
                title: string(o, "title")?,
                text: string(o, "text")?,
                url: string(o, "url")?,
            })
        }
        "post" | "put" | "upload" | "form" | "login" => {
            Err(format!("write attempt: {t:?} (workers are read-only GET)"))
        }
        "install" | "download" | "exec" | "run" | "shell" | "spawn" => Err(format!(
            "protocol: {t:?} is not something a worker can ask for"
        )),
        "remember" | "memory" | "write" | "note" | "config" | "setting" => {
            Err(format!("protocol: {t:?}: workers have no write access"))
        }
        other => Err(format!("protocol: unknown message type {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_five_shapes() {
        assert_eq!(parse(r#"{"t":"hb"}"#), Ok(Msg::Heartbeat));
        assert_eq!(parse(r#" {"t":"done"} "#), Ok(Msg::Done));
        assert_eq!(
            parse(r#"{"t":"get","url":"https://a.org/"}"#),
            Ok(Msg::Get {
                url: "https://a.org/".into()
            })
        );
        assert_eq!(parse(r#"{"t":"tokens","n":40}"#), Ok(Msg::Tokens(40)));
        assert!(matches!(
            parse(r#"{"t":"fact","title":"T","text":"x","url":"https://a.org/"}"#),
            Ok(Msg::Fact { .. })
        ));
    }

    #[test]
    fn read_only_no_post_forms_uploads_logins_cookies() {
        for l in [
            r#"{"t":"get","url":"https://a.org/","method":"POST"}"#,
            r#"{"t":"get","url":"https://a.org/","body":"x=1"}"#,
            r#"{"t":"get","url":"https://a.org/","form":{"a":1}}"#,
            r#"{"t":"get","url":"https://a.org/","cookie":"sid=1"}"#,
            r#"{"t":"get","url":"https://a.org/","headers":{"Referer":"x"}}"#,
            r#"{"t":"get","url":"https://a.org/","username":"ko"}"#,
            r#"{"t":"post","url":"https://a.org/"}"#,
            r#"{"t":"upload","url":"https://a.org/"}"#,
            r#"{"t":"login","url":"https://a.org/"}"#,
        ] {
            let e = parse(l).expect_err(l);
            assert!(e.starts_with("write attempt"), "{l}: {e}");
        }
    }

    #[test]
    fn no_install_exec_or_memory_write_messages() {
        for l in [
            r#"{"t":"install","pkg":"tor"}"#,
            r#"{"t":"download","url":"https://a.org/x.tar"}"#,
            r#"{"t":"exec","argv":["sh"]}"#,
            r#"{"t":"remember","text":"x"}"#,
            r#"{"t":"config","key":"learn.online"}"#,
            r#"{"t":"hb","extra":1}"#,
            "not json",
            "[1,2]",
        ] {
            assert!(parse(l).is_err(), "{l}");
        }
    }
}
