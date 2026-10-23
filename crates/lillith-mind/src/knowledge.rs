//! Lillith's knowledge and skills: local folders she can *read*, and the
//! drafting side of skill authoring (Phase 1 of the companion plan).
//!
//! Two folders live under her data dir (overridable in `[mind]`):
//!
//! - **knowledge** — reference facts she may quote.
//! - **skills** — short how-to recipes she may follow.
//!
//! On each chat turn [`KnowledgeBase::load`] indexes the markdown/text in
//! both, [`KnowledgeBase::select_within_budget`] scores them against the
//! user's message by keyword overlap (no network, no embeddings), and
//! [`KnowledgeBase::context_for`] renders the top matches into one block that
//! fits a rough token budget. The block is injected into the chat prompt.
//!
//! Authoring is read-first too: [`parse_skill_proposal`] turns a reply that
//! proposes a skill into a [`SkillDraft`], and [`SkillDraft::resolve_path`]
//! confines it to the skills folder. The draft is never written here — it is
//! handed to [`lillith_core::soul_contract`] for a Yes/No gate.

use lillith_core::config::MindConfig;
use lillith_core::soul_contract::PendingAction;
use std::path::{Path, PathBuf};

/// Which folder a document came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// Reference facts she may quote.
    Knowledge,
    /// How-to recipes she may follow.
    Skill,
}

impl DocKind {
    fn label(self) -> &'static str {
        match self {
            DocKind::Knowledge => "Knowledge",
            DocKind::Skill => "Skill",
        }
    }
}

/// One indexed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub path: PathBuf,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub kind: DocKind,
}

/// File extensions we index.
const TEXT_EXTS: &[&str] = &["md", "markdown", "txt"];

/// Rough token estimate (~4 characters per token).
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn has_text_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| TEXT_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Humanises a file stem for a fallback title (`my_cool-note` → `My cool note`).
fn title_from_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");
    let spaced = stem.replace(['_', '-'], " ");
    let mut c = spaced.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => "Untitled".into(),
    }
}

/// Parses a file's contents into a [`Doc`]: title from the first `# ` heading
/// (else the file name), summary from the first real line of prose.
pub fn parse_doc(path: &Path, contents: &str, kind: DocKind) -> Doc {
    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut in_fence = false;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.is_empty() {
            continue;
        }
        if title.is_none() {
            if let Some(h) = line.strip_prefix("# ") {
                title = Some(h.trim().to_string());
                continue;
            }
        }
        if summary.is_none() && !line.starts_with('#') {
            summary = Some(truncate_chars(line, 160));
        }
        if title.is_some() && summary.is_some() {
            break;
        }
    }
    Doc {
        path: path.to_path_buf(),
        title: title.unwrap_or_else(|| title_from_stem(path)),
        summary: summary.unwrap_or_default(),
        body: contents.trim().to_string(),
        kind,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Indexes the text files directly in `dir` (not recursive). A missing dir
/// yields no docs; unreadable files are skipped with a warning.
pub fn index_dir(dir: &Path, kind: DocKind) -> Vec<Doc> {
    let mut docs = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return docs,
        Err(e) => {
            tracing::warn!("knowledge: cannot read {}: {e}", dir.display());
            return docs;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !has_text_ext(&path) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => docs.push(parse_doc(&path, &contents, kind)),
            Err(e) => tracing::warn!("knowledge: cannot read {}: {e}", path.display()),
        }
    }
    // Stable order (by path) so selection ties are deterministic.
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    docs
}

/// The indexed knowledge + skills and the per-turn token budget.
#[derive(Debug, Clone)]
pub struct KnowledgeBase {
    docs: Vec<Doc>,
    budget_tokens: usize,
    enabled: bool,
}

impl KnowledgeBase {
    /// Loads from a [`MindConfig`]: indexes both folders (creating them if
    /// missing so they're there to drop files into). A disabled config, or
    /// one with no resolvable data dir, yields an empty, inert base.
    pub fn load(cfg: &MindConfig) -> Self {
        if !cfg.enabled {
            return Self::empty();
        }
        let mut docs = Vec::new();
        if let Some(kdir) = cfg.resolved_knowledge_dir() {
            ensure_dir(&kdir);
            docs.extend(index_dir(&kdir, DocKind::Knowledge));
        }
        if let Some(sdir) = cfg.resolved_skills_dir() {
            ensure_dir(&sdir);
            docs.extend(index_dir(&sdir, DocKind::Skill));
        }
        Self {
            docs,
            budget_tokens: cfg.context_budget(),
            enabled: true,
        }
    }

    /// An empty base (used when `[mind]` is disabled).
    pub fn empty() -> Self {
        Self {
            docs: Vec::new(),
            budget_tokens: 0,
            enabled: false,
        }
    }

    /// Builds directly from docs (tests).
    pub fn from_docs(docs: Vec<Doc>, budget_tokens: usize) -> Self {
        Self {
            docs,
            budget_tokens,
            enabled: true,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn docs(&self) -> &[Doc] {
        &self.docs
    }

    pub fn budget_tokens(&self) -> usize {
        self.budget_tokens
    }

    /// Scores every doc against `query` and returns the matches (score > 0),
    /// best first. Skills break ties ahead of knowledge, then path order.
    pub fn ranked(&self, query: &str) -> Vec<&Doc> {
        let terms = keywords(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u32, &Doc)> = self
            .docs
            .iter()
            .filter_map(|d| {
                let s = score_doc(d, &terms);
                (s > 0).then_some((s, d))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| skill_first(a.1.kind).cmp(&skill_first(b.1.kind)))
                .then_with(|| a.1.path.cmp(&b.1.path))
        });
        scored.into_iter().map(|(_, d)| d).collect()
    }

    /// The best matches whose rendered context fits `budget_tokens`, in rank
    /// order. Docs that would overflow the budget are dropped (a later, shorter
    /// match may still fit).
    pub fn select_within_budget(&self, query: &str, budget_tokens: usize) -> Vec<&Doc> {
        let mut chosen: Vec<&Doc> = Vec::new();
        for doc in self.ranked(query) {
            let mut trial = chosen.clone();
            trial.push(doc);
            if estimate_tokens(&render_context(&trial)) <= budget_tokens {
                chosen = trial;
            }
        }
        chosen
    }

    /// The reference block to inject for `query`, or `None` when nothing
    /// relevant fits. Uses the base's configured budget.
    pub fn context_for(&self, query: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let chosen = self.select_within_budget(query, self.budget_tokens);
        if chosen.is_empty() {
            return None;
        }
        Some(render_context(&chosen))
    }
}

fn ensure_dir(dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("knowledge: cannot create {}: {e}", dir.display());
    }
}

fn skill_first(kind: DocKind) -> u8 {
    match kind {
        DocKind::Skill => 0,
        DocKind::Knowledge => 1,
    }
}

/// Common function words that carry no topical signal; dropped from queries
/// so they don't match unrelated notes.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "you", "your", "that", "this", "with", "what", "how", "are", "was",
    "were", "can", "could", "would", "should", "about", "into", "from", "have", "has", "had",
    "want", "need", "make", "made", "please", "but", "not", "any", "all", "out", "get", "got",
    "let", "its", "it's",
];

/// Lowercased alphanumeric words of length ≥ 3 that aren't stopwords,
/// de-duplicated. These are the terms matched against the notes.
fn keywords(query: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for w in query
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(&w.as_str()))
    {
        if !seen.contains(&w) {
            seen.push(w);
        }
    }
    seen
}

/// Overlap score: whole-word hits in the title count triple, summary double,
/// body single (capped so a long doc can't dominate on one repeated word).
fn score_doc(doc: &Doc, terms: &[String]) -> u32 {
    let mut score = 0u32;
    for t in terms {
        score += 3 * count_words(&doc.title, t);
        score += 2 * count_words(&doc.summary, t);
        score += count_words(&doc.body, t).min(5);
    }
    score
}

/// Counts whole-word (case-insensitive) occurrences of `needle` in `haystack`.
/// Whole-word so "the" doesn't match inside "then".
fn count_words(haystack: &str, needle: &str) -> u32 {
    if needle.is_empty() {
        return 0;
    }
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.eq_ignore_ascii_case(needle))
        .count() as u32
}

/// Renders chosen docs into one injected block.
fn render_context(docs: &[&Doc]) -> String {
    let mut out = String::from(
        "Lillith's local notes. Use this reference material when relevant. \
Knowledge entries are facts you may quote; skills are how-to recipes you may \
follow. Do not invent sources beyond these.\n",
    );
    for d in docs {
        out.push_str(&format!(
            "\n## {}: {}\n{}\n",
            d.kind.label(),
            d.title,
            d.body
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Skill authoring (drafting side only; the write is gated by SoulContract).
// ---------------------------------------------------------------------------

/// A skill Lillith proposed, parsed out of a reply. Not written until a human
/// confirms it through the Soul Contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDraft {
    /// The sanitised `.md` file name (no directory components).
    pub filename: String,
    /// The markdown body to write.
    pub body: String,
}

impl SkillDraft {
    /// A draft from a raw (possibly unsafe) name and body. The name is
    /// sanitised to a bare `.md` file name.
    pub fn new(raw_name: &str, body: impl Into<String>) -> Self {
        Self {
            filename: sanitize_skill_filename(raw_name),
            body: body.into(),
        }
    }

    /// The absolute path this draft would be written to, confined to
    /// `skills_dir` (the sanitised file name cannot escape it).
    pub fn resolve_path(&self, skills_dir: &Path) -> PathBuf {
        skills_dir.join(&self.filename)
    }

    /// The gated action for this draft (hand to [`lillith_core::soul_contract`]).
    pub fn to_action(&self, skills_dir: &Path) -> PendingAction {
        PendingAction::WriteSkill {
            path: self.resolve_path(skills_dir),
            body: self.body.clone(),
        }
    }
}

/// Turns a raw name into a safe bare `.md` file name: only the final path
/// component, non-`[A-Za-z0-9._-]` collapsed to `-`, forced `.md`.
pub fn sanitize_skill_filename(raw: &str) -> String {
    // Keep only the last path component (defeat traversal / absolute paths).
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let mut cleaned = String::new();
    let mut last_dash = false;
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            cleaned.push(ch);
            last_dash = false;
        } else if !last_dash {
            cleaned.push('-');
            last_dash = true;
        }
    }
    let cleaned = cleaned.trim_matches(['-', '.', '_']);
    let stem = cleaned
        .strip_suffix(".md")
        .or_else(|| cleaned.strip_suffix(".markdown"))
        .unwrap_or(cleaned);
    let stem = stem.trim_matches(['-', '.', '_']);
    if stem.is_empty() {
        "skill.md".into()
    } else {
        format!("{stem}.md")
    }
}

/// Parses a skill proposal out of a reply. Recognised form: a fenced code
/// block whose info string is `skill` plus a file name — an opening fence
/// line of `` ```skill greet.md `` (or `` ```skill:greet.md ``), then the
/// markdown body, then a closing fence. Returns the first such block, or
/// `None` (a plain `` ```rust `` block, an unclosed fence, or an empty body
/// are not skills).
pub fn parse_skill_proposal(reply: &str) -> Option<SkillDraft> {
    let lines: Vec<&str> = reply.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start();
        if let Some(rest) = fence_info(line) {
            // rest is the info string after the fence, e.g. "skill greet.md".
            if let Some(name) = skill_fence_name(rest) {
                // Collect body until the closing fence.
                let mut body = String::new();
                let mut j = i + 1;
                let mut closed = false;
                while j < lines.len() {
                    let l = lines[j].trim_start();
                    if l.starts_with("```") || l.starts_with("~~~") {
                        closed = true;
                        break;
                    }
                    body.push_str(lines[j]);
                    body.push('\n');
                    j += 1;
                }
                let body = body.trim_matches('\n').to_string();
                if closed && !body.trim().is_empty() {
                    return Some(SkillDraft::new(name, body));
                }
            }
        }
        i += 1;
    }
    None
}

/// The info string after an opening code fence, or `None` if `line` isn't a fence.
fn fence_info(line: &str) -> Option<&str> {
    line.strip_prefix("```")
        .or_else(|| line.strip_prefix("~~~"))
}

/// If a fence info string marks a skill, returns the file name token.
fn skill_fence_name(info: &str) -> Option<&str> {
    let info = info.trim();
    let rest = info
        .strip_prefix("skill:")
        .or_else(|| info.strip_prefix("skill="))
        .or_else(|| info.strip_prefix("skill"))?;
    let name = rest.trim_start_matches([':', '=']).trim();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, title: &str, summary: &str, body: &str, kind: DocKind) -> Doc {
        Doc {
            path: PathBuf::from(path),
            title: title.into(),
            summary: summary.into(),
            body: body.into(),
            kind,
        }
    }

    #[test]
    fn parse_doc_pulls_title_and_summary() {
        let c = "# Coffee Brewing\n\nUse a 1:16 ratio of coffee to water.\nMore detail here.\n";
        let d = parse_doc(Path::new("/k/coffee.md"), c, DocKind::Knowledge);
        assert_eq!(d.title, "Coffee Brewing");
        assert_eq!(d.summary, "Use a 1:16 ratio of coffee to water.");
        assert_eq!(d.kind, DocKind::Knowledge);

        // No heading -> humanised file name; fenced code skipped for summary.
        let c2 = "```\ncode first\n```\nplain prose line\n";
        let d2 = parse_doc(Path::new("/k/my_note-two.txt"), c2, DocKind::Skill);
        assert_eq!(d2.title, "My note two");
        assert_eq!(d2.summary, "plain prose line");
    }

    #[test]
    fn index_dir_reads_only_text_files() {
        let dir = std::env::temp_dir().join(format!("lillith-kb-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "# A\nalpha\n").unwrap();
        std::fs::write(dir.join("b.txt"), "beta text\n").unwrap();
        std::fs::write(dir.join("c.png"), b"not text").unwrap();
        std::fs::write(dir.join("d.rs"), "fn main() {}").unwrap();
        let docs = index_dir(&dir, DocKind::Knowledge);
        assert_eq!(docs.len(), 2, "only .md and .txt");
        assert_eq!(docs[0].title, "A");
        assert_eq!(docs[1].title, "B"); // humanised from b.txt
                                        // Missing dir -> empty, no panic.
        assert!(index_dir(&dir.join("nope"), DocKind::Knowledge).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ranking_prefers_keyword_overlap_and_skills() {
        let kb = KnowledgeBase::from_docs(
            vec![
                doc(
                    "/k/git.md",
                    "Git rebase",
                    "rebase onto main",
                    "git rebase details",
                    DocKind::Knowledge,
                ),
                doc(
                    "/s/git.md",
                    "Git rebase recipe",
                    "how to rebase",
                    "step by step rebase",
                    DocKind::Skill,
                ),
                doc(
                    "/k/cook.md",
                    "Cooking",
                    "pasta",
                    "boil water",
                    DocKind::Knowledge,
                ),
            ],
            10_000,
        );
        let ranked = kb.ranked("how do I rebase in git?");
        assert!(!ranked.is_empty());
        // Both git docs match; the unrelated cooking doc does not.
        assert!(ranked
            .iter()
            .all(|d| d.title.to_lowercase().contains("git")));
        // A query with only short/no keywords selects nothing.
        assert!(kb.ranked("a an it").is_empty());
    }

    #[test]
    fn selection_respects_token_budget() {
        let body = "rebase the branch onto main carefully and keep history tidy".to_string();
        let one = || doc("/s/one.md", "rebase one", "rebase", &body, DocKind::Skill);
        let two = || {
            doc(
                "/s/two.md",
                "rebase two",
                "rebase",
                &body,
                DocKind::Knowledge,
            )
        };
        let kb = KnowledgeBase::from_docs(vec![one(), two()], 100_000);

        // Generous budget: both matching docs fit.
        assert_eq!(kb.select_within_budget("rebase", 100_000).len(), 2);

        // Cost (tokens) of injecting exactly one doc.
        let single = KnowledgeBase::from_docs(vec![one()], 100_000);
        let one_cost = estimate_tokens(&single.context_for("rebase").unwrap());

        // A budget that fits exactly one doc selects one (the top-ranked skill).
        let sel = kb.select_within_budget("rebase", one_cost);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].title, "rebase one");
        // One token short of a single doc: nothing fits.
        assert!(kb.select_within_budget("rebase", one_cost - 1).is_empty());

        // context_for honours the configured budget both ways.
        let too_tight = KnowledgeBase::from_docs(vec![one()], one_cost - 1);
        assert!(too_tight.context_for("rebase").is_none());
        let fits = KnowledgeBase::from_docs(vec![one()], one_cost);
        let ctx = fits.context_for("rebase").unwrap();
        assert!(ctx.contains("Skill: rebase one"));
        assert!(estimate_tokens(&ctx) <= one_cost);

        // Disabled base never injects.
        assert!(KnowledgeBase::empty().context_for("rebase").is_none());
    }

    #[test]
    fn sanitize_filename_blocks_traversal() {
        assert_eq!(sanitize_skill_filename("../../etc/passwd"), "passwd.md");
        assert_eq!(
            sanitize_skill_filename("/abs/path/My Skill.md"),
            "My-Skill.md"
        );
        assert_eq!(sanitize_skill_filename("note.markdown"), "note.md");
        assert_eq!(sanitize_skill_filename("weird@name!.md"), "weird-name.md");
        assert_eq!(sanitize_skill_filename(""), "skill.md");
        assert_eq!(sanitize_skill_filename("..."), "skill.md");
        // Resolved path stays inside the skills dir.
        let d = SkillDraft::new("../evil.md", "body");
        let p = d.resolve_path(Path::new("/home/ko/.local/share/lillith/skills"));
        assert_eq!(
            p,
            PathBuf::from("/home/ko/.local/share/lillith/skills/evil.md")
        );
    }

    #[test]
    fn parse_skill_proposal_reads_fenced_block() {
        let reply = "Sure, here's a skill for that~\n\n```skill greet.md\n# Greet\nSay hi warmly.\n```\n\nWant me to save it?";
        let d = parse_skill_proposal(reply).unwrap();
        assert_eq!(d.filename, "greet.md");
        assert_eq!(d.body, "# Greet\nSay hi warmly.");
        // Colon form works too.
        let d2 = parse_skill_proposal("```skill:notes.md\nbody line\n```").unwrap();
        assert_eq!(d2.filename, "notes.md");
        // A normal code block is not a skill.
        assert!(parse_skill_proposal("```rust\nfn main(){}\n```").is_none());
        // No reply, no draft.
        assert!(parse_skill_proposal("just chatting, no skills here").is_none());
        // Unclosed fence is ignored.
        assert!(parse_skill_proposal("```skill x.md\nno closing fence").is_none());

        let action = d.to_action(Path::new("/s"));
        assert_eq!(
            action,
            PendingAction::WriteSkill {
                path: PathBuf::from("/s/greet.md"),
                body: "# Greet\nSay hi warmly.".into(),
            }
        );
    }

    #[test]
    fn load_creates_folders_and_indexes() {
        let base = std::env::temp_dir().join(format!("lillith-kb-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cfg = MindConfig {
            enabled: true,
            knowledge_dir: base.join("k").to_string_lossy().into_owned(),
            skills_dir: base.join("s").to_string_lossy().into_owned(),
            max_context_tokens: 800,
            ..MindConfig::default()
        };
        // Folders don't exist yet; load creates them.
        let kb = KnowledgeBase::load(&cfg);
        assert!(base.join("k").is_dir() && base.join("s").is_dir());
        assert!(kb.docs().is_empty());
        assert_eq!(kb.budget_tokens(), 800);
        // Drop a file and reload.
        std::fs::write(
            base.join("s/brew.md"),
            "# Brew\nrebase nothing, just brew\n",
        )
        .unwrap();
        let kb = KnowledgeBase::load(&cfg);
        assert_eq!(kb.docs().len(), 1);
        assert_eq!(kb.docs()[0].kind, DocKind::Skill);
        // Disabled config -> inert.
        let off = MindConfig {
            enabled: false,
            ..cfg.clone()
        };
        assert!(!KnowledgeBase::load(&off).is_enabled());
        let _ = std::fs::remove_dir_all(&base);
    }
}
