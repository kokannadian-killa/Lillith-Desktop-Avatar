use super::*;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-bots-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn bot_in(kind: BotKind, dir: &Path) -> BotDef {
    BotDef {
        folders: vec![dir.display().to_string()],
        ..BotDef::stock(kind)
    }
}

#[test]
fn six_stock_demons_with_roles() {
    let r = stock_roster();
    let names: Vec<_> = r.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        ["Mammon", "Belphegor", "Paimon", "Baal", "Andras", "Naamah"]
    );
    assert_eq!(r[0].kind, BotKind::FileOrganizer);
    assert_eq!(r[5].kind.role(), "Lillith Pose Builder");
    assert!(r.iter().all(|b| b.enabled && !b.folders.is_empty()));
}

#[test]
fn roster_round_trips_and_renames_stick() {
    let mut r = stock_roster();
    r[0].name = "Moneybags".into();
    r[4].folders = vec!["/srv/a".into(), "~/b".into()];
    r[2].enabled = false;
    r.push(new_bot(BotKind::WebPageBuilder, &r));
    let d = tmp("roster");
    let p = d.join(ROSTER_FILE);
    save_roster(&p, &r).unwrap();
    let back = load_roster(&p).unwrap();
    assert_eq!(back, r);
    assert_eq!(back[6].name, "Asmodeus");
    assert_eq!(back[6].id, "bot-7");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn missing_roster_is_stock_and_unknown_kinds_skipped() {
    assert_eq!(
        load_roster(Path::new("/nonexistent/lillith/bots.toml")).unwrap(),
        stock_roster()
    );
    let r = parse_roster("[[bot]]\nkind = \"nope\"\n[[bot]]\nkind = \"red-team\"\nname = \"Andy\"\n")
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, "Andy");
    assert_eq!(r[0].id, "andras");
}

#[test]
fn confine_keeps_steps_inside_folders() {
    let d = tmp("confine");
    let f = vec![d.clone()];
    assert_eq!(confine(&f, Path::new("a/b.txt")).unwrap(), d.join("a/b.txt"));
    assert!(confine(&f, Path::new("../x")).is_err());
    assert!(confine(&f, &d.join("a/../../x")).is_err());
    assert!(confine(&f, Path::new("/etc/passwd")).is_err());
    assert!(confine(&[], Path::new("x")).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc", d.join("escape")).unwrap();
        assert!(confine(&f, &d.join("escape/passwd")).is_err());
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vet_drops_ops_the_kind_does_not_do() {
    let d = tmp("vet");
    let mammon = bot_in(BotKind::FileOrganizer, &d);
    let plan = Plan {
        steps: vec![
            Step::WriteFile {
                path: d.join("x"),
                body: "x".into(),
            },
            Step::Mkdir { path: d.join("Images") },
            Step::Run {
                command: "ls".into(),
                cwd: None,
                timeout_s: 5,
            },
        ],
        ..Default::default()
    };
    let (p, refused) = vet(&mammon, plan.clone());
    assert_eq!(p.steps.len(), 1);
    assert!(matches!(p.steps[0], Step::Mkdir { .. }));
    assert_eq!(refused.len(), 2);
    let andras = bot_in(BotKind::RedTeam, &d);
    let (p, _) = vet(&andras, plan);
    assert!(p.steps.iter().all(|s| matches!(s, Step::WriteFile { .. })));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn every_step_is_a_gated_action() {
    let j = Path::new("/j");
    let steps = [
        Step::WriteFile {
            path: "/a".into(),
            body: String::new(),
        },
        Step::Mkdir { path: "/a".into() },
        Step::Move {
            from: "/a".into(),
            to: "/b".into(),
        },
        Step::Copy {
            from: "/a".into(),
            to: "/b".into(),
        },
        Step::Run {
            command: "ls".into(),
            cwd: None,
            timeout_s: 1,
        },
        Step::Scaffold {
            dir: "/a".into(),
            language: Language::Rust,
            template: "bin".into(),
        },
        Step::Build {
            dir: "/a".into(),
            language: Language::Rust,
        },
    ];
    for s in steps {
        assert!(s.action(j).needs_confirmation(), "{s:?}");
    }
}

#[test]
fn model_plan_parses_from_prose_and_fences() {
    let text = "Sure!\n```json\n{\"say\": \"Here {you} go\", \"steps\": [\
        {\"op\": \"write_file\", \"path\": \"site/index.html\", \"body\": \"<h1>}</h1>\"},\
        {\"op\": \"run\", \"command\": \"ls\"},\
        {\"op\": \"scaffold\", \"dir\": \"app\", \"language\": \"python\"},\
        {\"op\": \"teleport\"}]}\n```";
    let p = parse_model_plan(text, Path::new("/base")).unwrap();
    assert_eq!(p.say, "Here {you} go");
    assert_eq!(p.steps.len(), 3);
    assert_eq!(
        p.steps[0],
        Step::WriteFile {
            path: "/base/site/index.html".into(),
            body: "<h1>}</h1>".into()
        }
    );
    assert!(matches!(&p.steps[1], Step::Run { cwd: Some(c), .. } if c == Path::new("/base")));
    assert!(matches!(
        &p.steps[2],
        Step::Scaffold {
            language: Language::Python,
            ..
        }
    ));
    assert!(parse_model_plan("no json here", Path::new("/b")).is_none());
}

#[test]
fn system_prompt_lists_only_allowed_ops() {
    let d = PathBuf::from("/w");
    let s = system_prompt(&bot_in(BotKind::WebPageBuilder, &d), &d);
    assert!(s.contains("write_file") && !s.contains("\"run\""), "{s}");
    assert!(s.contains("no internet"));
}

#[test]
fn recipes_plan_without_touching_disk() {
    let d = tmp("recipes");
    let p = recipes::plan(&bot_in(BotKind::WebPageBuilder, &d), "Ko's <b> landing page").unwrap();
    assert_eq!(p.steps.len(), 2);
    assert!(p.steps.iter().all(|s| s.paths()[0].starts_with(&d)));
    if let Step::WriteFile { body, .. } = &p.steps[0] {
        assert!(body.contains("&lt;b&gt;"), "{body}");
    }
    let p = recipes::plan(
        &bot_in(BotKind::ProgramBuilder, &d),
        "a python tool called hexdump",
    )
    .unwrap();
    assert!(matches!(
        &p.steps[0],
        Step::Scaffold { language: Language::Python, dir, .. } if dir.ends_with("hexdump")
    ));
    assert!(matches!(p.steps[1], Step::Build { .. }));
    let p = recipes::plan(&bot_in(BotKind::OsBuilder, &d), "phantomcore kde respin").unwrap();
    assert!(p.steps.iter().all(|s| matches!(s, Step::WriteFile { .. })));
    // Mammon on a messy folder: one organize plan.
    std::fs::write(d.join("a.png"), b"x").unwrap();
    std::fs::write(d.join("b.pdf"), b"x").unwrap();
    let p = recipes::plan(&bot_in(BotKind::FileOrganizer, &d), "tidy this").unwrap();
    assert!(matches!(&p.steps[..], [Step::Organize { plan }] if plan.len() >= 2));
    // Nothing was created by planning.
    let names: Vec<_> = std::fs::read_dir(&d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn andras_refuses_other_peoples_systems() {
    for job in [
        "scan https://example.com",
        "audit 8.8.8.8",
        "pentest bank.com",
        "check http://192.168.1.1/login",
    ] {
        assert!(recipes::targets_others(job).is_some(), "{job}");
    }
    for job in [
        "audit my projects",
        "check http://127.0.0.1:8080",
        "scan localhost:3000",
        "look at ~/Projects/app/config.toml",
    ] {
        assert!(recipes::targets_others(job).is_none(), "{job}");
    }
    let d = tmp("andras-refuse");
    let e = recipes::plan(&bot_in(BotKind::RedTeam, &d), "attack https://example.com").unwrap_err();
    assert!(e.contains("own machine"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn andras_audit_reports_by_name_only() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("andras");
    std::fs::write(d.join("id_rsa"), b"SECRET-CONTENT").unwrap();
    std::fs::set_permissions(d.join("id_rsa"), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(d.join("open.sh"), b"x").unwrap();
    std::fs::set_permissions(d.join("open.sh"), std::fs::Permissions::from_mode(0o666)).unwrap();
    let p = recipes::plan(&bot_in(BotKind::RedTeam, &d), "audit my stuff").unwrap();
    let all = p.notes.join("\n");
    assert!(all.contains("readable by others") && all.contains("id_rsa"), "{all}");
    assert!(all.contains("world-writable file") && all.contains("open.sh"), "{all}");
    match &p.steps[..] {
        [Step::WriteFile { path, body }] => {
            assert!(path.starts_with(&d));
            assert!(!body.contains("SECRET-CONTENT"));
        }
        s => panic!("{s:?}"),
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn naamah_needs_a_video() {
    let d = tmp("naamah");
    let e = recipes::plan(&bot_in(BotKind::PoseBuilder, &d), "import my clips").unwrap_err();
    assert!(e.contains("no videos"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn slugs_are_safe() {
    assert_eq!(recipes::slug("My Cool App!!", "x"), "my-cool-app");
    assert_eq!(recipes::slug("../../etc", "x"), "etc");
    assert_eq!(recipes::slug("!!!", "fallback"), "fallback");
}

#[test]
fn json_extraction_skips_broken_objects() {
    assert_eq!(
        extract_json("{oops} then {\"a\": 1}").unwrap()["a"],
        serde_json::json!(1)
    );
}
