use crate::model::{Severity, Warning};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum ContentKind {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "binary")]
    Binary,
    #[serde(rename = "non-utf8")]
    NonUtf8,
    #[serde(rename = "too-large")]
    TooLarge,
}

/// What kind of filesystem object a diff side is, derived from its git mode.
/// Surfaced separately from `ContentKind` because a symlink or gitlink can
/// render "text" content (the target path / `Subproject commit` line) while
/// being a fundamentally different kind of object than a regular file.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Regular,
    Executable,
    Symlink,
    Gitlink,
}

/// File type for a git mode string; `None` for an absent side ("000000" or
/// empty). Unknown modes conservatively map to `Regular`.
fn file_type_of_mode(mode: &str) -> Option<FileType> {
    if mode.is_empty() || mode.bytes().all(|b| b == b'0') {
        return None;
    }
    Some(match mode {
        "100755" => FileType::Executable,
        "120000" => FileType::Symlink,
        "160000" => FileType::Gitlink,
        _ => FileType::Regular,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change_kind: ChangeKind,
    pub content_kind: ContentKind,
    /// Git file mode（例 "100644", "100755", "120000", "160000"）。
    /// 存在しない側（added の old / deleted の new、mode "000000"）は None。
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// mode から導出したファイル種別。存在しない側は None。
    pub old_type: Option<FileType>,
    pub new_type: Option<FileType>,
    /// フル OID。zero-oid の側は None。`parse_unified_diff` 経由（demo）は常に None。
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
    /// blob サイズ（bytes）。gitlink・存在しない側・不明（demo 経由）は None。
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
    /// いずれかの側が Git LFS pointer blob（実データではなく pointer だけを
    /// レビューしていることを UI に明示するためのフラグ）。
    pub lfs_pointer: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// 内容が描画されない変更（binary / non-utf8 / too-large）。
    pub fn is_opaque(&self) -> bool {
        self.content_kind != ContentKind::Text
    }

    /// submit 時に明示 acknowledge が必要な変更。内容が描画されないもの
    /// （opaque）に加えて、レビュー画面の本文だけでは重大さが伝わらない
    /// 変更 — gitlink の pointer 変更（submodule pointer だけで中身の diff は
    /// 表示されない）、mode 変更（実行属性の付与・symlink 化など）— を含む。
    /// gitlink の同一 oid の pure rename は pointer が動いていないので対象外。
    pub fn requires_ack(&self) -> bool {
        if self.is_opaque() {
            return true;
        }
        let gitlink_involved =
            self.old_type == Some(FileType::Gitlink) || self.new_type == Some(FileType::Gitlink);
        if gitlink_involved && self.old_oid != self.new_oid {
            return true;
        }
        // Mode change with both sides present (e.g. 100644 -> 100755, or a
        // regular file becoming a symlink). Added/deleted files have only
        // one side and their kind is already the headline of the change.
        matches!((&self.old_mode, &self.new_mode), (Some(o), Some(n)) if o != n)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub section: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

/// Line-ending form of one diff line, preserved so the UI can make an
/// LF→CRLF (or newline-at-EOF) change visible instead of showing two
/// identical-looking strings.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    Lf,
    Crlf,
    /// No trailing newline (the last line of a file without a final newline).
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    /// 表示 content からは改行が取り除かれているため、元の行末形式を別途
    /// 保持する。demo の unified-diff parse 経由では常に `Lf`（原文の行末
    /// 形式が diff テキストからは分からないため）。
    pub eol: Eol,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// Parse the `a/<old> b/<new>` remainder of a `diff --git ` line, used as
/// the provisional path source (overridden later by `--- `/`+++ ` or
/// `rename from`/`rename to` lines). Returns `None` for either side if the
/// expected `a/`/`b/` structure isn't found (e.g. unusual paths).
fn parse_diff_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(old) = rest.strip_prefix("a/") {
        if let Some(idx) = old.find(" b/") {
            let old_path = old[..idx].to_string();
            let new_path = old[idx + 3..].to_string();
            return (Some(old_path), Some(new_path));
        }
    }
    (None, None)
}

/// Strip a leading `a/` or `b/` prefix from a diff path, as used in
/// `--- a/foo` / `+++ b/foo` lines. `/dev/null` is not touched here;
/// callers check for it separately.
fn strip_ab_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

/// Parse a path from a `--- ` / `+++ ` header line's remainder.
/// Returns `None` for `/dev/null`, otherwise the path with `a/`/`b/` stripped.
fn parse_diff_path(rest: &str) -> Option<String> {
    // Git may append a tab and extra info (e.g. timestamps) after the path;
    // only the part before a tab is the path itself.
    let path_part = rest.split('\t').next().unwrap_or(rest).trim_end();
    if path_part == "/dev/null" {
        None
    } else {
        Some(strip_ab_prefix(path_part))
    }
}

/// Parse a unified hunk header of the form:
/// `@@ -{old_start}[,{old_count}] +{new_start}[,{new_count}] @@[ section]`
/// Returns (old_start, old_count, new_start, new_count, section).
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, section) = match rest.split_once(" @@") {
        Some((r, s)) => (r, s.trim_start().to_string()),
        None => return None,
    };
    let mut parts = ranges.split_whitespace();
    let old_range = parts.next()?;
    let new_range = parts.next()?;

    let (old_start, old_count) = parse_range(old_range.strip_prefix('-')?)?;
    let (new_start, new_count) = parse_range(new_range.strip_prefix('+')?)?;

    Some((old_start, old_count, new_start, new_count, section))
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

pub fn parse_unified_diff(input: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let (old_path, new_path) = parse_diff_git_paths(rest);
            files.push(FileDiff {
                old_path,
                new_path,
                change_kind: ChangeKind::Modified,
                content_kind: ContentKind::Text,
                old_mode: None,
                new_mode: None,
                old_type: None,
                new_type: None,
                old_oid: None,
                new_oid: None,
                old_size: None,
                new_size: None,
                lfs_pointer: false,
                hunks: Vec::new(),
            });
            continue;
        }

        let Some(file) = files.last_mut() else {
            // Content before any `diff --git` line; nothing to attach it to.
            continue;
        };

        // `--- `/`+++ ` file headers only appear before the first hunk of a
        // file; once hunks have started, lines with these prefixes are hunk
        // content (e.g. a removed line whose content starts with "-- ").
        if file.hunks.is_empty() {
            if let Some(rest) = line.strip_prefix("--- ") {
                file.old_path = parse_diff_path(rest);
                continue;
            }
            if let Some(rest) = line.strip_prefix("+++ ") {
                file.new_path = parse_diff_path(rest);
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("new file mode ") {
            file.change_kind = ChangeKind::Added;
            file.new_mode = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("deleted file mode ") {
            file.change_kind = ChangeKind::Deleted;
            file.old_mode = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename from ") {
            file.change_kind = ChangeKind::Renamed;
            file.old_path = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename to ") {
            file.change_kind = ChangeKind::Renamed;
            file.new_path = Some(rest.to_string());
            continue;
        }
        if (line.starts_with("Binary files") && line.ends_with("differ"))
            || line.starts_with("GIT binary patch")
        {
            file.content_kind = ContentKind::Binary;
            continue;
        }

        if let Some((old_start, old_count, new_start, new_count, section)) = parse_hunk_header(line)
        {
            old_no = old_start;
            new_no = new_start;
            file.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                section,
                lines: Vec::new(),
            });
            continue;
        }

        if line.starts_with('\\') {
            // "\ No newline at end of file" marker; not a content line.
            continue;
        }

        let Some(hunk) = file.hunks.last_mut() else {
            continue;
        };

        let mut chars = line.chars();
        let marker = chars.next();
        let content = chars.as_str().to_string();

        match marker {
            // A genuinely blank line in a hunk still counts as context;
            // `chars.next()` returns `None` for it rather than `Some(' ')`.
            Some(' ') | None => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Context,
                    content,
                    eol: Eol::Lf,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                });
                old_no += 1;
                new_no += 1;
            }
            Some('-') => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Remove,
                    content,
                    eol: Eol::Lf,
                    old_no: Some(old_no),
                    new_no: None,
                });
                old_no += 1;
            }
            Some('+') => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Add,
                    content,
                    eol: Eol::Lf,
                    old_no: None,
                    new_no: Some(new_no),
                });
                new_no += 1;
            }
            Some(_) => {
                // Unrecognized marker character; not a valid diff content
                // line, so skip it rather than misclassifying it.
            }
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_diff_with_content_kind(content_kind: ContentKind) -> FileDiff {
        FileDiff {
            old_path: None,
            new_path: None,
            change_kind: ChangeKind::Modified,
            content_kind,
            old_mode: None,
            new_mode: None,
            old_type: None,
            new_type: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            lfs_pointer: false,
            hunks: Vec::new(),
        }
    }

    #[test]
    fn budget_line_violation_detection() {
        let tight = ResourceBudget {
            max_file_lines: 3,
            max_line_bytes: 8,
            ..ResourceBudget::default()
        };
        // Too many lines wins first.
        let w = line_budget_violation("f.txt", "1\n2\n3\n4\n", "", &tight).unwrap();
        assert_eq!(w.code, "FILE_TOO_MANY_LINES");
        // Line length violation on either side.
        let w = line_budget_violation("f.txt", "short\n", "waaaaay too long\n", &tight).unwrap();
        assert_eq!(w.code, "LINE_TOO_LONG");
        // Within budget.
        assert!(line_budget_violation("f.txt", "ok\n", "fine\n", &tight).is_none());
    }

    #[test]
    fn gitlink_requires_ack_only_when_pointer_moves() {
        let gitlink = |old_oid: &str, new_oid: &str| FileDiff {
            old_type: Some(FileType::Gitlink),
            new_type: Some(FileType::Gitlink),
            old_mode: Some("160000".to_string()),
            new_mode: Some("160000".to_string()),
            old_oid: Some(old_oid.to_string()),
            new_oid: Some(new_oid.to_string()),
            ..file_diff_with_content_kind(ContentKind::Text)
        };
        // Pointer moved -> ack; same-oid pure rename -> nothing hidden.
        assert!(gitlink("aaaa", "bbbb").requires_ack());
        assert!(!gitlink("aaaa", "aaaa").requires_ack());
        // Added gitlink: one side absent counts as a pointer move.
        let added = FileDiff {
            old_oid: None,
            ..gitlink("aaaa", "bbbb")
        };
        assert!(added.requires_ack());
    }

    #[test]
    fn is_opaque_true_only_for_non_text_content() {
        assert!(!file_diff_with_content_kind(ContentKind::Text).is_opaque());
        assert!(file_diff_with_content_kind(ContentKind::Binary).is_opaque());
        assert!(file_diff_with_content_kind(ContentKind::NonUtf8).is_opaque());
        assert!(file_diff_with_content_kind(ContentKind::TooLarge).is_opaque());
    }

    const MODIFIED: &str = "\
diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,5 @@ header-one
 line1
-old2
+new2
+new3
 line4
@@ -10,3 +11,3 @@ header-two
 a
-b
+B
 c
";

    #[test]
    fn parses_modified_file_with_two_hunks() {
        let files = parse_unified_diff(MODIFIED);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.new_path.as_deref(), Some("src/app.ts"));
        assert_eq!(f.change_kind, ChangeKind::Modified);
        assert_eq!(f.hunks.len(), 2);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 4, 1, 5)
        );
        assert_eq!(h.section, "header-one");
        // line numbering
        assert_eq!(h.lines[0].old_no, Some(1));
        assert_eq!(h.lines[0].new_no, Some(1));
        assert_eq!(h.lines[1].kind as u8, LineKind::Remove as u8);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[1].new_no, None);
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[3].new_no, Some(3));
        assert_eq!(h.lines[4].old_no, Some(3));
        assert_eq!(h.lines[4].new_no, Some(4));
    }

    const ADDED: &str = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
";

    #[test]
    fn parses_added_file() {
        let f = &parse_unified_diff(ADDED)[0];
        assert_eq!(f.change_kind, ChangeKind::Added);
        assert_eq!(f.old_path, None);
        assert_eq!(f.new_path.as_deref(), Some("new.txt"));
        assert_eq!(f.hunks[0].new_count, 2);
    }

    const DELETED: &str = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 4444444..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-now
";

    #[test]
    fn parses_deleted_file() {
        let f = &parse_unified_diff(DELETED)[0];
        assert_eq!(f.change_kind, ChangeKind::Deleted);
        assert_eq!(f.new_path, None);
        assert_eq!(f.hunks[0].new_count, 0);
        assert_eq!(f.hunks[0].lines[0].new_no, None);
    }

    const RENAME_NO_CHANGE: &str = "\
diff --git a/old_name.rs b/new_name.rs
similarity index 100%
rename from old_name.rs
rename to new_name.rs
";

    #[test]
    fn parses_pure_rename_without_hunks() {
        let f = &parse_unified_diff(RENAME_NO_CHANGE)[0];
        assert_eq!(f.change_kind, ChangeKind::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("old_name.rs"));
        assert_eq!(f.new_path.as_deref(), Some("new_name.rs"));
        assert!(f.hunks.is_empty());
    }

    const BINARY: &str = "\
diff --git a/logo.png b/logo.png
index 5555555..6666666 100644
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn parses_binary_file() {
        let f = &parse_unified_diff(BINARY)[0];
        assert_eq!(f.content_kind, ContentKind::Binary);
        assert!(f.hunks.is_empty());
        assert_eq!(f.new_path.as_deref(), Some("logo.png"));
    }

    #[test]
    fn skips_no_newline_marker() {
        let d = "\
diff --git a/x b/x
index 1..2 100644
--- a/x
+++ b/x
@@ -1 +1 @@
-a
\\ No newline at end of file
+b
\\ No newline at end of file
";
        let f = &parse_unified_diff(d)[0];
        assert_eq!(f.hunks[0].lines.len(), 2);
    }

    #[test]
    fn multiple_files() {
        let combined = format!("{MODIFIED}{ADDED}");
        assert_eq!(parse_unified_diff(&combined).len(), 2);
    }

    #[test]
    fn header_like_content_lines_are_not_misparsed() {
        // A removed line whose content starts with "-- " renders as "--- ..."
        // and an added line whose content starts with "++ " renders as
        // "+++ ..." — these must be treated as hunk content, not file headers.
        let d = "\
diff --git a/query.sql b/query.sql
index 1111111..2222222 100644
--- a/query.sql
+++ b/query.sql
@@ -1,3 +1,3 @@
 SELECT 1;
--- old comment
+++ new comment
 SELECT 2;
";
        let files = parse_unified_diff(d);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.old_path.as_deref(), Some("query.sql"));
        assert_eq!(f.new_path.as_deref(), Some("query.sql"));
        let h = &f.hunks[0];
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind as u8, LineKind::Context as u8);
        assert_eq!(h.lines[1].kind as u8, LineKind::Remove as u8);
        assert_eq!(h.lines[1].content, "-- old comment");
        assert_eq!(h.lines[2].kind as u8, LineKind::Add as u8);
        assert_eq!(h.lines[2].content, "++ new comment");
        assert_eq!(h.lines[3].kind as u8, LineKind::Context as u8);
        assert_eq!(h.lines[3].old_no, Some(3));
        assert_eq!(h.lines[3].new_no, Some(3));
    }
}

#[derive(Debug)]
pub enum GitError {
    NotARepo,
    BadBase(String),
    GitFailed(String),
    /// The diff exceeds the changed-file-count budget and reviewing it in
    /// one session would be meaningless (byte/line budgets never produce
    /// this error — they degrade individual files to `TooLarge` instead).
    BudgetExceeded(String),
    /// A git subprocess's stdout exceeded its per-call cap (see
    /// [`wait_with_timeout`]) and was killed rather than read without
    /// bound. Distinct from [`GitError::GitFailed`] so callers can tell
    /// "git misbehaved/was killed for a resource reason" apart from an
    /// ordinary failure, though both currently exit with the same code.
    OutputOverflow(String),
}

/// Hard deadline for any single git subprocess this module spawns. Git on a
/// local repository finishes in milliseconds; a command still running after
/// this long is wedged (stalled filesystem, misbehaving fsmonitor, a
/// tampered `git` on PATH) and is killed rather than allowed to hold the
/// review process open.
pub const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Extra time given to a subprocess (and its process group) to actually die
/// and be reaped after [`wait_with_timeout`] sends the kill signal. This is
/// not part of the caller-visible deadline budget — it bounds how long the
/// *kill itself* is allowed to take. If the direct child hasn't been
/// reaped within this window of a successful kill signal, something is
/// badly wrong (e.g. an uninterruptible D-state process) and that is
/// reported as a fatal error rather than silently retried forever.
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Default stdout cap for git subprocess calls whose output is inherently
/// small (rev-parse, ls-files, merge-base, current-branch, cat-file
/// --batch-check, ...). Generous headroom over anything legitimate; exists
/// so a wedged or hostile `git` can't grow this process's memory without
/// bound while its output is buffered.
const DEFAULT_MAX_STDOUT_BYTES: usize = 8 * 1024 * 1024;

/// `git status --porcelain=v2 -z --untracked-files=all` can legitimately
/// list many thousands of paths in a large, messy worktree.
const STATUS_MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;

/// Entry-count cap for [`parse_status_v2_z`], well above what any normal
/// (even very messy) worktree produces. Bounds the number of `String`s the
/// dirty gate allocates and enumerates; paired with
/// [`STATUS_MAX_STDOUT_BYTES`], which bounds the raw buffer those entries
/// are parsed from. On overflow the worktree is reported dirty (never
/// clean) with a summary instead of enumerating every path — see
/// [`WorktreeStatus::overflow`].
const STATUS_MAX_ENTRIES: usize = 10_000;

/// `git diff-tree --raw` lists every changed path plus both blob
/// oids/modes — the largest legitimate output of any call this module
/// makes, and the one bound tightly by `budget.max_files` rather than by
/// content size.
const DIFF_TREE_RAW_MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;

/// Ring-buffer cap for stderr: git errors are almost always at the end of
/// the message, so on overflow the *tail* is kept, not the head.
const STDERR_CAP: usize = 8 * 1024;

/// Spawns `cmd`, placing it in its own process group on unix
/// (`process_group(0)`, i.e. pgid = the child's own pid) *before* the fork
/// happens. This must be set at spawn time — a [`std::process::Child`]
/// can't be moved into a new group after the fact — so every call site that
/// eventually hands its child to [`wait_with_timeout`] must spawn through
/// this (or set the option itself) for that function's process-group kill
/// to reach the right processes. On non-unix this is a plain `cmd.spawn()`;
/// there is no portable process-group API, so timeout kills there stay
/// limited to the direct child (see [`kill_child_group`]).
fn spawn_grouped(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// Result of the bounded stdout reader thread in [`wait_with_timeout`].
struct CappedRead {
    data: Vec<u8>,
    /// `true` if more than `cap` bytes arrived — reading stopped at `cap`
    /// rather than continuing, so `data.len() == cap` in that case.
    overflowed: bool,
    /// `Some` if the read loop stopped because of a genuine I/O error
    /// (anything other than a clean `Ok(0)` EOF or a retried
    /// `ErrorKind::Interrupted`). `data` in that case is a SHORT read, not
    /// a complete one, and [`wait_with_timeout`] must not treat it as a
    /// successful, complete `Output` — see the doc comment on
    /// [`read_stdout_capped`].
    read_error: Option<std::io::Error>,
}

const READ_CHUNK_BYTES: usize = 64 * 1024;

/// Reads `pipe` to EOF, capped at `cap` bytes. Reads in fixed-size chunks
/// (rather than `read_to_end`) so the cap can be enforced without ever
/// buffering more than `cap + READ_CHUNK_BYTES` bytes, and stops the moment
/// the cap is exceeded instead of draining the rest of a hostile/runaway
/// stream.
///
/// `ErrorKind::Interrupted` (`EINTR`) is retried in place, matching what
/// `read_to_end` used to do before this loop replaced it. Any OTHER read
/// error is reported via `read_error` rather than silently treated as EOF:
/// a short buffer from a genuine error is not a complete, trustworthy
/// response, and returning it as one would silently truncate the diff
/// (contradicting this module's "nothing is silently truncated"
/// guarantee) — see [`wait_with_timeout`], which fails the whole call
/// instead of returning a truncated success when `read_error` is set.
fn read_stdout_capped(pipe: Option<std::process::ChildStdout>, cap: usize) -> CappedRead {
    use std::io::Read;
    let mut data = Vec::new();
    let mut overflowed = false;
    let mut read_error = None;
    if let Some(mut pipe) = pipe {
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(data.len());
                    if n <= room {
                        data.extend_from_slice(&chunk[..n]);
                    } else {
                        data.extend_from_slice(&chunk[..room]);
                        overflowed = true;
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    read_error = Some(e);
                    break;
                }
            }
        }
    }
    CappedRead {
        data,
        overflowed,
        read_error,
    }
}

/// Result of the bounded stderr reader thread in [`wait_with_timeout`]. See
/// [`CappedRead::read_error`] — the same "a genuine read error is not EOF"
/// rule applies here.
struct CappedStderr {
    data: Vec<u8>,
    read_error: Option<std::io::Error>,
}

/// Reads `pipe` to EOF into a ring buffer capped at `cap` bytes, keeping the
/// *tail* (most recent bytes) rather than the head — git error messages are
/// almost always at the end of stderr, so on overflow that's what's worth
/// keeping. Unlike [`read_stdout_capped`] this never stops early on a full
/// buffer: stderr has no success-path consumer waiting on it, so draining
/// it fully (memory bounded, unlike the old unbounded `Vec`) is simplest
/// and lets the pipe's write end close normally.
///
/// As in [`read_stdout_capped`], `ErrorKind::Interrupted` is retried and
/// any other read error is reported via `read_error` instead of being
/// treated as a clean EOF.
fn read_stderr_tail(pipe: Option<std::process::ChildStderr>, cap: usize) -> CappedStderr {
    use std::io::Read;
    let mut ring: std::collections::VecDeque<u8> = std::collections::VecDeque::with_capacity(cap);
    let mut read_error = None;
    if let Some(mut pipe) = pipe {
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    for &b in &chunk[..n] {
                        if ring.len() == cap {
                            ring.pop_front();
                        }
                        ring.push_back(b);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    read_error = Some(e);
                    break;
                }
            }
        }
    }
    CappedStderr {
        data: ring.into_iter().collect(),
        read_error,
    }
}

/// Kills `child`'s whole process group with `SIGKILL` on unix (relies on
/// the child having been spawned via [`spawn_grouped`], so its pgid equals
/// its own pid) so a descendant that has escaped `wait_with_timeout`'s
/// direct-child tracking — e.g. a shell backgrounding a process that
/// inherits and holds a pipe open — dies too, instead of being left to wedge
/// the reader threads forever. `ESRCH` (group already empty — everything in
/// it already exited) is not an error; any other failure is propagated
/// rather than swallowed, per the "kill failures are fatal, not ignored"
/// requirement. Never signals pid 0 or a negative pid (which `kill(2)`
/// interprets as "every process this user can reach" / "every process in
/// this group" respectively) — `child.id()` is always a real positive pid
/// for a process we just spawned, but the guard is kept explicit rather
/// than relying on that invariant silently.
#[cfg(unix)]
fn kill_child_group(child: &mut std::process::Child) -> std::io::Result<()> {
    let pid = child.id();
    if pid <= 1 {
        return Ok(());
    }
    // SAFETY: `kill` with a negative pid targets the process group whose
    // id is the pid's absolute value; no memory is touched, only a signal
    // is sent. `pid` was validated `> 1` above and fits in `libc::pid_t`
    // because it came from `Child::id`.
    let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err);
        }
    }
    Ok(())
}

/// No portable process-group API outside unix; best-effort direct-child
/// kill only, matching this module's pre-existing non-unix behavior.
#[cfg(not(unix))]
fn kill_child_group(child: &mut std::process::Child) -> std::io::Result<()> {
    child.kill()
}

/// Reaps `child`, waiting up to `until` for it to actually exit after being
/// killed. A kill signal is not synchronous — the kernel schedules the
/// death — so this still polls rather than assuming the next `try_wait`
/// succeeds. If `child` is not reaped by `until`, that is treated as a
/// fatal condition (the process refused to die, e.g. stuck in
/// uninterruptible I/O) rather than retried indefinitely.
fn reap_child(
    child: &mut std::process::Child,
    until: std::time::Instant,
) -> std::io::Result<std::process::ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if std::time::Instant::now() >= until {
            return Err(std::io::Error::other(format!(
                "git subprocess did not exit within {}s of being killed",
                KILL_GRACE.as_secs()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Waits for `child` (spawned via [`spawn_grouped`], with piped stdout and
/// stderr) up to `timeout`, draining both pipes from helper threads so a
/// chatty child can't deadlock against a full pipe.
///
/// This has to satisfy two guarantees the old join-after-wait structure
/// didn't: (1) stdout is bounded at `max_stdout_bytes`, and (2) the *whole
/// call* returns within `timeout` (plus the bounded [`KILL_GRACE`] a kill
/// needs to land) even if a descendant of `child` — not `child` itself —
/// is still holding a pipe open, which is exactly the scenario where the
/// old code's unconditional `.join()` on the reader threads could block
/// forever. The single poll loop below checks child-exited, stdout-ready,
/// and stderr-ready every 5ms via non-blocking `try_wait`/`try_recv` (never
/// a blocking join) and breaks the moment all three are satisfied, the
/// stdout cap is exceeded, or `timeout` elapses. Anything other than "all
/// three satisfied, no overflow" falls through to the kill path: the whole
/// process group is killed (reaching a pipe-holding descendant even though
/// only the direct `child` is tracked), the direct child is reaped under a
/// bounded secondary grace period, and a descriptive error is returned —
/// never partial success, so callers can't mistake a killed run for a
/// complete one. That includes a genuine pipe read error (anything but a
/// retried `ErrorKind::Interrupted`): [`read_stdout_capped`] and
/// [`read_stderr_tail`] report those via `read_error` rather than treating
/// them as EOF, and this loop breaks out on that exactly like it does on
/// overflow, so a `read_error` can never be mistaken for a complete,
/// successful `Output`.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
    max_stdout_bytes: usize,
) -> std::io::Result<std::process::Output> {
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdout_tx.send(read_stdout_capped(stdout_pipe, max_stdout_bytes));
    });
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = stderr_tx.send(read_stderr_tail(stderr_pipe, STDERR_CAP));
    });

    let deadline = std::time::Instant::now() + timeout;
    let mut status: Option<std::process::ExitStatus> = None;
    let mut stdout_read: Option<CappedRead> = None;
    let mut stderr_read: Option<CappedStderr> = None;
    let mut overflowed = false;
    let mut read_failed = false;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if stdout_read.is_none() {
            if let Ok(r) = stdout_rx.try_recv() {
                overflowed = r.overflowed;
                read_failed |= r.read_error.is_some();
                stdout_read = Some(r);
            }
        }
        if stderr_read.is_none() {
            if let Ok(r) = stderr_rx.try_recv() {
                read_failed |= r.read_error.is_some();
                stderr_read = Some(r);
            }
        }
        if overflowed || read_failed {
            break;
        }
        match (status, stdout_read.take(), stderr_read.take()) {
            (Some(status), Some(stdout_read), Some(stderr_read)) => {
                return Ok(std::process::Output {
                    status,
                    stdout: stdout_read.data,
                    stderr: stderr_read.data,
                });
            }
            // Not all three are ready yet: put back whichever ones `take`
            // pulled out so the next iteration doesn't lose them.
            (_, taken_stdout, taken_stderr) => {
                stdout_read = taken_stdout;
                stderr_read = taken_stderr;
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Something is still outstanding at `deadline` (the child itself, a
    // descendant holding a pipe open, or the stdout cap was hit): kill the
    // whole group and reap the direct child within a bounded grace period.
    // Reader threads are deliberately not waited on here — the group kill
    // closes the pipes almost immediately, they'll finish on their own,
    // and this call must not block past `deadline + KILL_GRACE` either way.
    kill_child_group(&mut child)?;
    if status.is_none() {
        let reap_deadline = std::time::Instant::now() + KILL_GRACE;
        reap_child(&mut child, reap_deadline)?;
    }
    if overflowed {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("git subprocess stdout exceeded {max_stdout_bytes} bytes and was killed"),
        ));
    }
    if read_failed {
        // A short buffer from a genuine read error must never be reported
        // as a complete, successful `Output` — see the doc comment above
        // and on `CappedRead::read_error`. Prefer the stdout error detail
        // when both pipes failed; either is enough to explain the failure.
        let detail = stdout_read
            .and_then(|r| r.read_error)
            .or_else(|| stderr_read.and_then(|r| r.read_error))
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown read error".to_string());
        return Err(std::io::Error::other(format!(
            "git subprocess pipe read failed before EOF, output cannot be trusted: {detail}"
        )));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "git subprocess exceeded {}s and was killed",
            timeout.as_secs()
        ),
    ))
}

/// Runs `cmd` to completion under a caller-supplied `timeout`, with stdin
/// closed and stdout capped at `max_stdout_bytes` (stderr is always capped
/// at [`STDERR_CAP`], see [`wait_with_timeout`]). Shared by [`timed_output`]
/// (the [`GIT_TIMEOUT`] default, used by every plumbing call in this module
/// except the one below) and [`rev_parse_commit_with_deadline`] (a shorter,
/// caller-chosen deadline for the submit-path freshness check, which must
/// fail fast enough to stay under the server's own request timeout).
fn timed_output_with_deadline(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    max_stdout_bytes: usize,
) -> std::io::Result<std::process::Output> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    wait_with_timeout(spawn_grouped(&mut cmd)?, timeout, max_stdout_bytes)
}

/// Runs `cmd` to completion under [`GIT_TIMEOUT`] with stdin closed and
/// stdout capped at `max_stdout_bytes`.
fn timed_output(
    cmd: std::process::Command,
    max_stdout_bytes: usize,
) -> std::io::Result<std::process::Output> {
    timed_output_with_deadline(cmd, GIT_TIMEOUT, max_stdout_bytes)
}

/// Maps a [`wait_with_timeout`]/[`timed_output`] I/O error to a [`GitError`],
/// distinguishing the stdout-overflow case (`ErrorKind::FileTooLarge`, see
/// [`wait_with_timeout`]) from every other failure (spawn error, timeout,
/// fatal kill/reap failure), which all become [`GitError::GitFailed`].
fn map_wait_err(e: std::io::Error) -> GitError {
    if e.kind() == std::io::ErrorKind::FileTooLarge {
        GitError::OutputOverflow(e.to_string())
    } else {
        GitError::GitFailed(e.to_string())
    }
}

/// Repo root of cwd, or `NotARepo`. Uses `git rev-parse --show-toplevel`.
pub fn repo_root() -> Result<std::path::PathBuf, GitError> {
    let mut cmd = base_git();
    cmd.args(["rev-parse", "--show-toplevel"]);
    // Only a clean non-zero exit means "not a repository"; a spawn failure
    // or a killed-at-deadline git is a git problem and must not be
    // misreported as "run this from inside a repo".
    let output = timed_output(cmd, DEFAULT_MAX_STDOUT_BYTES).map_err(map_wait_err)?;
    if !output.status.success() {
        return Err(GitError::NotARepo);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(path))
}

/// Structured worktree cleanliness report, from `git status --porcelain=v2
/// -z --untracked-files=all`. Every path is repo-root-relative.
///
/// The categories matter differently for review completeness:
/// - `tracked_changes`: staged or unstaged edits to tracked files — the
///   committed diff under review does not include them.
/// - `untracked`: files git has never seen. The classic miss is a brand-new
///   file the agent forgot to `git add`: the review looks complete while the
///   new file is reviewed nowhere.
/// - `submodules_dirty`: submodules whose worktree differs from the recorded
///   pointer; the parent diff can't show what changed inside.
#[derive(Debug, Default)]
pub struct WorktreeStatus {
    pub tracked_changes: Vec<String>,
    pub untracked: Vec<String>,
    pub submodules_dirty: Vec<String>,
    /// Set when [`parse_status_v2_z`] stopped early because the entry count
    /// exceeded [`STATUS_MAX_ENTRIES`], instead of enumerating every
    /// remaining path. When set, the three lists above are a **partial**
    /// enumeration (only the entries seen before the cap) — their absence
    /// of an item must never be read as "not dirty". `is_clean` always
    /// returns `false` while this is set: fail-closed, never a false
    /// "clean" from a status report that was cut short.
    pub overflow: Option<String>,
}

impl WorktreeStatus {
    pub fn is_clean(&self) -> bool {
        self.overflow.is_none()
            && self.tracked_changes.is_empty()
            && self.untracked.is_empty()
            && self.submodules_dirty.is_empty()
    }
}

/// Reports the worktree's cleanliness, untracked files included. `-c
/// core.fsmonitor=false` disables a repo-local fsmonitor hook (which could
/// otherwise short-circuit or lie about the working-tree scan) and
/// `--ignore-submodules=none` overrides any repo-local `.gitmodules`/config
/// setting that would hide dirty submodules — same hardened,
/// don't-trust-repo-local-config posture as [`base_git`]'s env scrubbing.
/// `-z` keeps paths verbatim (no quoting), and porcelain v2 is parsed
/// structurally instead of by line prefix guessing.
pub fn worktree_status(root: &std::path::Path) -> Result<WorktreeStatus, GitError> {
    let mut cmd = git_cmd(root);
    cmd.args([
        "-c",
        "core.fsmonitor=false",
        "status",
        "--porcelain=v2",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ]);
    let output = timed_output(cmd, STATUS_MAX_STDOUT_BYTES).map_err(map_wait_err)?;
    if !output.status.success() {
        return Err(GitError::GitFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    parse_status_v2_z(&output.stdout)
}

/// Parses `git status --porcelain=v2 -z` output. Entry types:
/// - `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>` — ordinary change
/// - `2 <XY> <sub> ... <X><score> <path>\0<origPath>` — rename/copy (the
///   original path is the NEXT NUL-delimited token, consumed here)
/// - `u ...` — unmerged (counted as a tracked change)
/// - `? <path>` — untracked, `! <path>` — ignored (ignored are skipped)
///
/// The `<sub>` field is `N...` for a non-submodule and `S<c><m><u>` for a
/// submodule; any of c/m/u being set means the submodule worktree differs
/// from the recorded pointer. Fails closed on malformed entries: a status
/// this function cannot understand must not silently read as "clean".
fn parse_status_v2_z(bytes: &[u8]) -> Result<WorktreeStatus, GitError> {
    let mut status = WorktreeStatus::default();
    let mut tokens = bytes
        .split(|&b| b == 0)
        .filter(|t| !t.is_empty())
        .peekable();
    let mut entry_count: usize = 0;
    while let Some(token) = tokens.next() {
        // Checked before this record's fields are parsed at all, so a
        // worktree with an enormous number of entries stops doing work at
        // the cap instead of first parsing/allocating every path and only
        // then discovering the gate can't enumerate them all. A rename's
        // second (`origPath`) token is consumed as part of the same
        // logical entry below and does not bump this counter again.
        entry_count += 1;
        if entry_count > STATUS_MAX_ENTRIES {
            status.overflow = Some(format!(
                "git status reports more than {STATUS_MAX_ENTRIES} entries; worktree treated as dirty without enumerating every path"
            ));
            return Ok(status);
        }
        let text = std::str::from_utf8(token)
            .map_err(|_| GitError::GitFailed("non-UTF-8 git status entry".to_string()))?;
        let malformed = || GitError::GitFailed(format!("unparseable git status entry: {text:?}"));
        let (kind, rest) = text.split_once(' ').ok_or_else(malformed)?;
        match kind {
            "1" | "2" => {
                // Fields: XY sub mH mI mW hH hI [Xscore] path — path is
                // everything after the fixed-count fields (it may contain
                // spaces), so split off exactly the field count.
                let field_count = if kind == "1" { 7 } else { 8 };
                let mut rest_fields = rest;
                let mut sub = "";
                for i in 0..field_count {
                    let (field, tail) = rest_fields.split_once(' ').ok_or_else(malformed)?;
                    if i == 1 {
                        sub = field;
                    }
                    rest_fields = tail;
                }
                let path = rest_fields;
                if path.is_empty() {
                    return Err(malformed());
                }
                // `S<c><m><u>`: `c` = commit pointer moved (an ordinary,
                // committable tracked change), `m`/`u` = the submodule's own
                // worktree is dirty inside — content the parent diff can
                // never show.
                let submodule_dirty_inside =
                    sub.starts_with('S') && (sub.contains('M') || sub.contains('U'));
                if submodule_dirty_inside {
                    status.submodules_dirty.push(path.to_string());
                } else {
                    status.tracked_changes.push(path.to_string());
                }
                if kind == "2" {
                    // Consume the rename's original path token.
                    tokens.next().ok_or_else(malformed)?;
                }
            }
            "u" => {
                // Unmerged: 8 fixed fields then path.
                let mut rest_fields = rest;
                for _ in 0..9 {
                    let (_, tail) = rest_fields.split_once(' ').ok_or_else(malformed)?;
                    rest_fields = tail;
                }
                status.tracked_changes.push(rest_fields.to_string());
            }
            "?" => status.untracked.push(rest.to_string()),
            "!" => {}
            _ => return Err(malformed()),
        }
    }
    Ok(status)
}

/// Current branch name (for `--title` default). `git rev-parse --abbrev-ref HEAD`;
/// on any failure returns `"review"`.
pub fn current_branch(root: &std::path::Path) -> String {
    let mut cmd = git_cmd(root);
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"]);
    match timed_output(cmd, DEFAULT_MAX_STDOUT_BYTES) {
        Ok(output) if output.status.success() => {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                "review".to_string()
            } else {
                branch
            }
        }
        _ => "review".to_string(),
    }
}

/// Largest single blob that will be rendered inline. Files with a bigger
/// blob on either side are reported as `ContentKind::TooLarge` with a warning.
pub const MAX_FILE_BYTES: usize = 1_048_576;

/// Total blob-content budget for one diff. Once exceeded, remaining files
/// are reported as `ContentKind::TooLarge` with a warning rather than being
/// silently truncated.
pub const MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;

/// Every hard resource limit the diff pipeline enforces, in one place. The
/// posture is bounded-refuse, never unbounded-process: inputs inside the
/// budget render fully; a file over a per-file limit — or past a whole-review
/// byte/line budget — degrades to an explicitly acknowledged `TooLarge` card
/// with a structured warning; only a diff over `max_files` refuses to start
/// (`GitError::BudgetExceeded`). Nothing is ever silently truncated.
#[derive(Debug, Clone)]
pub struct ResourceBudget {
    /// Maximum changed files in one review; beyond this the review refuses
    /// to start (a human cannot meaningfully review it in one sitting, and
    /// rendering it would melt the browser).
    pub max_files: usize,
    /// Largest single blob rendered inline; larger degrades to `TooLarge`.
    pub max_file_bytes: usize,
    /// Total blob budget; once exceeded remaining files degrade to `TooLarge`.
    pub max_total_bytes: usize,
    /// Maximum lines on either side of a file's text diff; more degrades to
    /// `TooLarge` (bounds line-diff CPU as well as DOM size).
    pub max_file_lines: usize,
    /// Longest single line rendered; a file with a longer line degrades to
    /// `TooLarge` (a multi-megabyte minified line would freeze the browser).
    pub max_line_bytes: usize,
    /// Total rendered diff lines across the whole review; files past the
    /// budget degrade to `TooLarge` (bounds the session JSON and the DOM).
    pub max_total_lines: usize,
    /// Total claimed changed-line registrations across every concern's
    /// resolved locations (`mapping::resolve_mapping`). Changed lines
    /// themselves are already bounded by `max_total_lines`, but a concern
    /// can re-claim the same large file's lines, and up to 200 concerns can
    /// each do so — this bounds that cross-concern multiplication. Once
    /// exceeded the review refuses to start (`GitError::BudgetExceeded`)
    /// rather than building an oversized session.
    pub max_resolved_edges: usize,
    /// Total `HunkRef` entries across every concern's displayed hunks
    /// (`mapping::resolve_mapping`). Bounds the size of the mapping (and the
    /// session JSON it feeds) independent of `max_resolved_edges`, since a
    /// concern's hunk list is deduplicated per `(file, hunk)` pair but still
    /// scales with distinct hunks touched across up to 200 concerns. Once
    /// exceeded the review refuses to start (`GitError::BudgetExceeded`).
    pub max_hunk_refs: usize,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        ResourceBudget {
            max_files: 2000,
            max_file_bytes: MAX_FILE_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_file_lines: 50_000,
            max_line_bytes: 64 * 1024,
            max_total_lines: 200_000,
            max_resolved_edges: 1_000_000,
            max_hunk_refs: 100_000,
        }
    }
}

/// Result of [`compute_diff`]: the per-file diffs, non-fatal warnings (e.g.
/// files skipped because they exceed size limits), and the resolved commit
/// endpoints the diff was computed between — kept so the session can pin its
/// result to exactly these commits and detect a moved `HEAD` at submit time.
#[derive(Debug)]
pub struct DiffOutput {
    pub files: Vec<FileDiff>,
    pub warnings: Vec<Warning>,
    /// Full oid the user-supplied base ref resolved to.
    pub base_oid: String,
    /// Full oid `HEAD` resolved to when the diff was computed.
    pub head_oid: String,
    /// Full oid of `merge-base(base, HEAD)` — the diff's left side.
    pub merge_base_oid: String,
}

/// One record of `git diff-tree -r -z --raw` output.
#[derive(Debug)]
struct RawEntry {
    old_mode: String,
    new_mode: String,
    old_oid: String,
    new_oid: String,
    status: char,
    path: String,
    path2: Option<String>,
}

/// How a raw entry will be rendered, decided before blob contents are fetched.
enum Plan {
    /// Identical oids on both sides (pure rename, mode-only change, or a
    /// gitlink pointing at the same commit on both sides): no hunks, no
    /// content needed.
    NoContent,
    /// Over a size limit: `ContentKind::TooLarge`, no hunks.
    TooLarge,
    /// Diff two text representations. Gitlink (mode 160000) sides never have
    /// their oid fetched (it usually doesn't exist locally as a blob);
    /// instead they contribute the synthetic `Subproject commit <oid>` line.
    /// See [`side_bytes`].
    Content,
}

/// Environment variables that can redirect which repository/objects git
/// reads, alter diff output, or inject arbitrary git config. The reviewed
/// agent shares this process environment, so all of them are stripped
/// before running git.
const SCRUBBED_GIT_ENV: &[&str] = &[
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    // GIT_CONFIG_PARAMETERS can itself encode arbitrary key/value pairs
    // (including fake GIT_CONFIG_KEY_<n>/GIT_CONFIG_VALUE_<n> style
    // injection) that git decodes directly, independent of the count var.
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    // GIT_CONFIG_KEY_<n>/GIT_CONFIG_VALUE_<n> are dynamically named and
    // cannot be enumerated here, but git only reads them up to
    // GIT_CONFIG_COUNT, so scrubbing that count var alone neutralizes them.
    // A fake shallow boundary can cut ancestry short; scrubbing this leaves
    // only the repo's real shallow state (if any) in effect.
    "GIT_SHALLOW_FILE",
    // Overrides the ref storage backend on newer git; scrubbing is free
    // defense in depth even on git versions that don't support it yet.
    "GIT_REFERENCE_BACKEND",
];

/// Base `git` invocation: replacement refs disabled (an in-repo agent can
/// `git replace` HEAD with an innocent-looking commit), commit grafts
/// disabled (`$GIT_DIR/info/grafts` / `GIT_GRAFT_FILE` can rewrite commit
/// parentage and move the merge-base so the diff collapses; unlike replace
/// refs, `--no-replace-objects` does not cover grafts — pinning the graft
/// file to `/dev/null` overrides the default path and disables repo-local
/// grafts too), repo-redirection env stripped, and `LC_ALL=C` for stable
/// message parsing.
fn base_git() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("--no-replace-objects")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .env("LC_ALL", "C");
    for var in SCRUBBED_GIT_ENV {
        cmd.env_remove(var);
    }
    cmd
}

/// Base `git` invocation for this module, scoped to `root` via `-C`. See
/// [`base_git`] for the hardening this applies (replacement refs disabled,
/// repo-redirection env scrubbed). All commands used here are plumbing that
/// ignores diff drivers anyway, but removing them is cheap defense in depth.
fn git_cmd(root: &std::path::Path) -> std::process::Command {
    let mut cmd = base_git();
    cmd.arg("-C").arg(root);
    cmd
}

fn run_git(
    root: &std::path::Path,
    args: &[&str],
    max_stdout_bytes: usize,
) -> Result<std::process::Output, GitError> {
    let mut cmd = git_cmd(root);
    cmd.args(args);
    timed_output(cmd, max_stdout_bytes).map_err(map_wait_err)
}

/// Canonicalized, absolute paths of the repository's git directory and (for a
/// worktree checkout) the shared common git directory, via `git rev-parse
/// --git-dir --git-common-dir`. Callers use this to keep `--out` from landing
/// inside git's own bookkeeping (e.g. `.git/result.json`).
///
/// `git rev-parse` prints these relative to the directory `-C` pointed at
/// unless `GIT_DIR` forces an absolute path; `base_git` scrubs `GIT_DIR` from
/// the environment, so relative output is joined onto `root` before
/// canonicalizing. A path that fails to canonicalize (unexpected, since the
/// git dir must exist for `root` to be a repo at all) is dropped rather than
/// causing the whole call to fail — callers only use this list for a
/// containment check, and a dropped entry only means that one entry can't
/// reject anything (the other entry, if canonicalizable, still can).
pub(crate) fn git_dirs(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, GitError> {
    let output = run_git(
        root,
        &["rev-parse", "--git-dir", "--git-common-dir"],
        DEFAULT_MAX_STDOUT_BYTES,
    )?;
    if !output.status.success() {
        return Err(GitError::GitFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut dirs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = std::path::PathBuf::from(line);
        let abs = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        if let Ok(canon) = std::fs::canonicalize(&abs) {
            dirs.push(canon);
        }
    }
    Ok(dirs)
}

/// Whether `rel_path` (a repo-root-relative, `/`-separated git pathspec) is
/// tracked in the index, via `git ls-files --error-unmatch`: success (exit 0)
/// means tracked.
pub(crate) fn is_tracked(root: &std::path::Path, rel_path: &str) -> Result<bool, GitError> {
    let output = run_git(
        root,
        &["ls-files", "--error-unmatch", "--", rel_path],
        DEFAULT_MAX_STDOUT_BYTES,
    )?;
    Ok(output.status.success())
}

/// Resolves `<rev>^{commit}` to a full oid.
///
/// Distinguishes *why* it failed: if git itself couldn't be run at all (spawn
/// error — e.g. the binary is missing), that's `GitFailed` regardless of
/// which rev was being resolved. If git ran and exited non-zero (the rev
/// doesn't resolve), that's `BadBase` here; callers resolving `HEAD` remap
/// that to `GitFailed` since `HEAD` is never the user-supplied base.
pub(crate) fn rev_parse_commit(root: &std::path::Path, rev: &str) -> Result<String, GitError> {
    rev_parse_commit_with_deadline(root, rev, GIT_TIMEOUT)
}

/// Same as [`rev_parse_commit`], under a caller-chosen `timeout` instead of
/// the [`GIT_TIMEOUT`] default. `check_head_freshness` in `server.rs` uses
/// this with a short deadline: the timeout hierarchy for a submit must be
/// internal-deadline < server request timeout < client timeout, so this
/// single rev-parse (the only git call on the submit path) has to fail well
/// inside the server's own request timeout rather than sharing the generous
/// 60s budget the initial diff computation gets.
pub(crate) fn rev_parse_commit_with_deadline(
    root: &std::path::Path,
    rev: &str,
    timeout: std::time::Duration,
) -> Result<String, GitError> {
    let mut cmd = git_cmd(root);
    cmd.args([
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{rev}^{{commit}}"),
    ]);
    let output =
        timed_output_with_deadline(cmd, timeout, DEFAULT_MAX_STDOUT_BYTES).map_err(map_wait_err)?;
    if !output.status.success() {
        return Err(GitError::BadBase(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_zero_oid(oid: &str) -> bool {
    !oid.is_empty() && oid.bytes().all(|b| b == b'0')
}

fn is_octal_mode_field(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (b'0'..=b'7').contains(&b))
}

fn is_hex_oid_field(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Converts a NUL-delimited path token to UTF-8, failing closed instead of
/// lossily collapsing invalid bytes to `�`: two different non-UTF-8 paths
/// would otherwise render identically, making concern mapping and comment
/// anchors ambiguous.
fn utf8_path(token: &[u8], malformed: &impl Fn(&str) -> GitError) -> Result<String, GitError> {
    std::str::from_utf8(token).map(str::to_string).map_err(|_| {
        malformed(&format!(
            "non-UTF-8 path {:?} (ronten requires UTF-8 paths)",
            String::from_utf8_lossy(token)
        ))
    })
}

/// Parses `git diff-tree -r -z --raw` output:
/// `:<oldmode> <newmode> <oldoid> <newoid> <status>\0<path>\0[<path2>\0]`.
/// Paths are NUL-delimited so they arrive verbatim (no quoting/escaping).
/// Any structurally malformed record is a hard error: this parser feeds the
/// review gate, so partial success is worse than failing the whole diff.
///
/// Unbounded: used only by tests that exercise the field-level parsing
/// directly. Production code goes through [`parse_raw_z_capped`], which adds
/// the file-count early-exit.
#[cfg(test)]
fn parse_raw_z(bytes: &[u8]) -> Result<Vec<RawEntry>, GitError> {
    parse_raw_z_impl(bytes, None)
}

/// [`parse_raw_z`], but bails out with [`GitError::BudgetExceeded`] the
/// moment a `max_entries + 1`th record would start, instead of parsing the
/// whole raw stream and checking the count afterward. This bounds both the
/// parse work and the `Vec<RawEntry>` allocation to `max_entries` records
/// regardless of how many more the raw `-z` stream actually contains — the
/// stream itself is separately bounded by
/// [`DIFF_TREE_RAW_MAX_STDOUT_BYTES`], but without this a diff touching
/// millions of files would still fully parse (and allocate a `RawEntry` for
/// every one of them) before being refused.
fn parse_raw_z_capped(bytes: &[u8], max_entries: usize) -> Result<Vec<RawEntry>, GitError> {
    parse_raw_z_impl(bytes, Some(max_entries))
}

fn parse_raw_z_impl(bytes: &[u8], max_entries: Option<usize>) -> Result<Vec<RawEntry>, GitError> {
    let malformed =
        |detail: &str| GitError::GitFailed(format!("unexpected diff-tree output: {detail}"));
    let mut tokens = bytes.split(|&b| b == 0).peekable();
    let mut entries = Vec::new();
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            // An empty token is only legal as the very last one (the
            // trailing NUL after the final record); anything after it would
            // desynchronize the field/path token stream.
            if tokens.peek().is_some() {
                return Err(malformed("unexpected empty token before end of output"));
            }
            continue;
        }
        // Checked before any field parsing of this record so a diff with
        // millions of changed files stops doing work at the boundary
        // instead of fully validating/allocating every record first and
        // only then discovering it's over budget.
        if let Some(max_entries) = max_entries {
            if entries.len() >= max_entries {
                return Err(GitError::BudgetExceeded(format!(
                    "diff touches more than {max_entries} files; review it in smaller pieces (narrower --base or split the change)"
                )));
            }
        }
        let meta = String::from_utf8_lossy(token).to_string();
        let Some(meta) = meta.strip_prefix(':') else {
            return Err(malformed(&format!(
                "record does not start with ':': {meta:?}"
            )));
        };
        // The `-z` raw format packs the rename/copy score into field 5
        // itself (e.g. `R100`), so a well-formed record is always exactly 5
        // space-separated fields.
        let parts: Vec<&str> = meta.split(' ').collect();
        if parts.len() != 5 {
            return Err(malformed(&format!(
                "record has {} fields (expected 5): {meta:?}",
                parts.len()
            )));
        }
        let (old_mode, new_mode, old_oid, new_oid, status_field) =
            (parts[0], parts[1], parts[2], parts[3], parts[4]);
        if !is_octal_mode_field(old_mode) || !is_octal_mode_field(new_mode) {
            return Err(malformed(&format!("non-octal mode field: {meta:?}")));
        }
        if !is_hex_oid_field(old_oid) || !is_hex_oid_field(new_oid) {
            return Err(malformed(&format!("non-hex oid field: {meta:?}")));
        }
        let mut status_chars = status_field.chars();
        let status = status_chars
            .next()
            .ok_or_else(|| malformed("empty status field"))?;
        if !matches!(status, 'A' | 'D' | 'M' | 'R' | 'C' | 'T' | 'U' | 'X') {
            return Err(malformed(&format!("unknown status letter: {meta:?}")));
        }
        if !status_chars.as_str().bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed(&format!(
                "invalid rename/copy score in status field: {meta:?}"
            )));
        }
        let path_token = tokens
            .next()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| malformed(&format!("record {meta:?} has no path")))?;
        let path = utf8_path(path_token, &malformed)?;
        let path2 = if matches!(status, 'R' | 'C') {
            let token = tokens.next().filter(|t| !t.is_empty()).ok_or_else(|| {
                malformed(&format!("rename/copy record {meta:?} has no second path"))
            })?;
            Some(utf8_path(token, &malformed)?)
        } else {
            None
        };
        entries.push(RawEntry {
            old_mode: old_mode.to_string(),
            new_mode: new_mode.to_string(),
            old_oid: old_oid.to_string(),
            new_oid: new_oid.to_string(),
            status,
            path,
            path2,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod raw_tests {
    use super::*;

    #[test]
    fn parse_raw_z_valid_record() {
        let raw = b":100644 100755 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0a.txt\0";
        let entries = parse_raw_z(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].status, 'M');
    }

    #[test]
    fn parse_raw_z_rejects_garbage_meta() {
        // A token that isn't `:`-prefixed is not a diff-tree raw record;
        // silently skipping it would desynchronize the path tokens.
        assert!(parse_raw_z(b"garbage\0a.txt\0").is_err());
    }

    #[test]
    fn parse_raw_z_rejects_truncated_record() {
        // Meta token with no following path token.
        assert!(parse_raw_z(b":100644 100644 111 222 M\0").is_err());
        // Rename record missing its second path.
        assert!(parse_raw_z(
            b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R90\0old.txt\0"
        )
        .is_err());
        // Meta with fewer than 5 fields.
        assert!(parse_raw_z(b":100644 100644 M\0a.txt\0").is_err());
    }

    #[test]
    fn parse_raw_z_rejects_bad_fields() {
        // 6 fields
        assert!(parse_raw_z(b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M extra\0a.txt\0").is_err());
        // non-octal mode
        assert!(parse_raw_z(b":10z644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0a.txt\0").is_err());
        // non-hex oid
        assert!(parse_raw_z(
            b":100644 100644 zzzz 2222222222222222222222222222222222222222 M\0a.txt\0"
        )
        .is_err());
        // unknown status letter
        assert!(parse_raw_z(b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 Q\0a.txt\0").is_err());
        // interior empty token
        assert!(parse_raw_z(b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0a.txt\0\0b.txt\0").is_err());
    }

    #[test]
    fn parse_raw_z_rejects_non_utf8_path() {
        let raw = b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0\x80path\0";
        let err = parse_raw_z(raw).unwrap_err();
        let GitError::GitFailed(msg) = err else {
            panic!("expected GitFailed")
        };
        assert!(msg.contains("non-UTF-8"), "message should explain: {msg}");
    }

    #[test]
    fn parse_raw_z_rejects_non_utf8_second_path() {
        let raw = b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R90\0ok.txt\0\x81new\0";
        assert!(parse_raw_z(raw).is_err());
    }

    /// Builds `n` well-formed `M` (modify) raw records with distinct paths.
    fn valid_records(n: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        for i in 0..n {
            buf.extend_from_slice(
                format!(":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0file{i}.txt\0")
                    .as_bytes(),
            );
        }
        buf
    }

    #[test]
    fn diff_tree_exactly_at_budget_passes() {
        // The boundary case: a diff touching exactly `max_files` files must
        // not be refused, only one strictly over the limit.
        let raw = valid_records(2000);
        let entries = parse_raw_z_capped(&raw, 2000).unwrap();
        assert_eq!(entries.len(), 2000);
    }

    #[test]
    fn diff_tree_refuses_at_2001_without_parsing_all() {
        // 2000 valid records followed by one that is structurally garbage.
        // If the capped parser fully parsed every record before checking
        // the count, it would trip over the garbage record and return
        // `GitFailed`. Getting `BudgetExceeded` instead proves parsing
        // stopped at the 2001st record without ever looking at its
        // contents.
        let mut raw = valid_records(2000);
        raw.extend_from_slice(b"not-a-valid-record\0trailing.txt\0");
        match parse_raw_z_capped(&raw, 2000) {
            Err(GitError::BudgetExceeded(msg)) => {
                assert!(msg.contains("2000"), "unexpected message: {msg}");
            }
            other => panic!("expected BudgetExceeded (early bail), got {other:?}"),
        }
    }

    #[test]
    fn diff_tree_capped_parse_is_bounded_not_just_the_final_check() {
        // A much larger stream (10x the cap) still bails promptly rather
        // than allocating a `RawEntry` for every record; this is a
        // regression guard on the *mechanism* (checked per-record, not
        // only compared against `entries.len()` at the very end).
        let raw = valid_records(20_000);
        let entries = match parse_raw_z_capped(&raw, 2000) {
            Err(GitError::BudgetExceeded(_)) => return,
            Ok(entries) => entries,
            Err(other) => panic!("unexpected error: {other:?}"),
        };
        panic!(
            "expected BudgetExceeded, got Ok with {} entries",
            entries.len()
        );
    }
}

#[cfg(test)]
mod blob_of_tests {
    use super::*;

    #[test]
    fn blob_of_missing_oid_is_an_error() {
        let contents = std::collections::HashMap::new();
        assert!(blob_of(&contents, "0000000000000000000000000000000000000000").is_ok());
        assert!(blob_of(&contents, "1234567890123456789012345678901234567890").is_err());
    }
}

fn is_gitlink_mode(mode: &str) -> bool {
    mode == "160000"
}

/// `Some(mode)` unless `mode` is the "no such side" sentinel `"000000"`
/// (added's old side, deleted's new side).
fn mode_opt(mode: &str) -> Option<String> {
    if mode == "000000" {
        None
    } else {
        Some(mode.to_string())
    }
}

/// `Some(oid)` unless `oid` is the all-zero sentinel (no such side).
fn oid_opt(oid: &str) -> Option<String> {
    if is_zero_oid(oid) {
        None
    } else {
        Some(oid.to_string())
    }
}

/// Blob size for one side of an entry, looked up from the `--batch-check`
/// results. `None` for a nonexistent side (zero oid) or a gitlink (never
/// queried, since its oid is a commit in another repo, not a blob here).
fn blob_size(
    sizes: &std::collections::HashMap<String, usize>,
    mode: &str,
    oid: &str,
) -> Option<u64> {
    if is_zero_oid(oid) || is_gitlink_mode(mode) {
        return None;
    }
    sizes.get(oid).map(|&size| size as u64)
}

/// Path to show in user-facing warnings: the post-change path when present.
fn display_path(entry: &RawEntry) -> &str {
    entry.path2.as_deref().unwrap_or(&entry.path)
}

fn entry_paths(entry: &RawEntry) -> (Option<String>, Option<String>) {
    match entry.status {
        'A' => (None, Some(entry.path.clone())),
        'D' => (Some(entry.path.clone()), None),
        'R' | 'C' => (Some(entry.path.clone()), entry.path2.clone()),
        _ => (Some(entry.path.clone()), Some(entry.path.clone())),
    }
}

fn entry_change_kind(entry: &RawEntry) -> ChangeKind {
    match entry.status {
        'A' => ChangeKind::Added,
        'D' => ChangeKind::Deleted,
        'R' => ChangeKind::Renamed,
        'C' => ChangeKind::Copied,
        _ => ChangeKind::Modified,
    }
}

/// Joins `writer` (the stdin-writing helper thread started by
/// [`cat_file_stdin`]) without ever blocking past `grace` from the moment
/// this is called. A plain, unconditional `.join()` here would reintroduce
/// exactly the bug `wait_with_timeout`'s process-group kill was written to
/// avoid, from the write side instead of the read side: a tampered `git`
/// can exit 0 (so `wait_with_timeout` legitimately returns `Ok` — the
/// direct child exited, stdout/stderr both EOF'd) while a descendant it
/// spawned keeps the stdin *read* end open without ever reading it. On
/// that success path nothing kills the process group, so `write_all` on a
/// full pipe blocks forever and a `--batch` request for a large diff
/// (tens to hundreds of KB of oids, past the OS pipe buffer) reaches that
/// exact deadlock.
///
/// Instead this polls [`std::thread::JoinHandle::is_finished`] on a short
/// interval — mirroring the non-blocking `try_wait`/`try_recv` poll loop in
/// [`wait_with_timeout`] — and if the writer still hasn't finished by
/// `grace`, drops the handle (detaching the thread, the same policy
/// `wait_with_timeout` already applies to its own reader threads on its
/// timeout path: they are never joined, only left to finish or die on
/// their own once whatever was blocking them goes away) and reports a
/// failure instead of blocking further. On the normal fast path — the
/// writer already finished by the time `wait_with_timeout` returns, which
/// is the overwhelmingly common case since a cat-file request is at most a
/// few hundred KB — `is_finished()` is true on the very first check, so
/// this returns immediately, same as the old unconditional join.
fn join_writer_bounded(
    writer: std::thread::JoinHandle<std::io::Result<()>>,
    grace: std::time::Duration,
) -> Result<std::io::Result<()>, GitError> {
    let deadline = std::time::Instant::now() + grace;
    loop {
        if writer.is_finished() {
            return writer.join().map_err(|_| {
                GitError::GitFailed("cat-file stdin writer thread panicked".to_string())
            });
        }
        if std::time::Instant::now() >= deadline {
            drop(writer);
            return Err(GitError::GitFailed(
                "git cat-file stdin writer did not drain within deadline".to_string(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Runs `git cat-file <flag>` feeding `input` (one oid per line) on stdin
/// and returning raw stdout, capped at `max_stdout_bytes`. Stdin is written
/// from a helper thread so a large response can't deadlock against a full
/// stdin pipe. A write failure (e.g. git exiting early) means the response
/// git did produce is for a truncated request, not a complete one, so it
/// must not be trusted. The writer thread is joined via
/// [`join_writer_bounded`], not a raw blocking `.join()` — see its doc
/// comment for why an unbounded join here is itself a hang bug.
fn cat_file_stdin(
    root: &std::path::Path,
    flag: &str,
    input: &str,
    max_stdout_bytes: usize,
) -> Result<Vec<u8>, GitError> {
    use std::io::Write;
    let mut cmd = git_cmd(root);
    cmd.args(["cat-file", flag])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = spawn_grouped(&mut cmd).map_err(map_wait_err)?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let input = input.to_string();
    let writer =
        std::thread::spawn(move || -> std::io::Result<()> { stdin.write_all(input.as_bytes()) });
    let output = wait_with_timeout(child, GIT_TIMEOUT, max_stdout_bytes).map_err(map_wait_err)?;
    let write_result = join_writer_bounded(writer, KILL_GRACE)?;
    if !output.status.success() {
        return Err(GitError::GitFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    write_result
        .map_err(|e| GitError::GitFailed(format!("failed to write cat-file stdin request: {e}")))?;
    Ok(output.stdout)
}

/// Headroom, per requested oid, added to a `cat-file` stdout cap on top of
/// the content-size budget the caller already tracks — covers the
/// `<oid> <type> <size>\n` header git prepends to every object (batch) or
/// emits per line (batch-check), which isn't counted against that budget.
const CAT_FILE_HEADER_HEADROOM_PER_OID: usize = 128;

/// Object sizes via `git cat-file --batch-check`. Oids reported `missing`
/// are simply absent from the map (callers treat that as an error). The
/// response is one short header line per oid, so the cap only needs to
/// scale with the request size, not with any content budget.
fn blob_sizes(
    root: &std::path::Path,
    oids: &[String],
) -> Result<std::collections::HashMap<String, usize>, GitError> {
    let mut sizes = std::collections::HashMap::new();
    if oids.is_empty() {
        return Ok(sizes);
    }
    let mut input = oids.join("\n");
    input.push('\n');
    let cap = oids
        .len()
        .saturating_mul(CAT_FILE_HEADER_HEADROOM_PER_OID)
        .max(DEFAULT_MAX_STDOUT_BYTES);
    let out = cat_file_stdin(root, "--batch-check", &input, cap)?;
    for line in String::from_utf8_lossy(&out).lines() {
        let parts: Vec<&str> = line.split(' ').collect();
        if parts.len() == 3 {
            if let Ok(size) = parts[2].parse() {
                sizes.insert(parts[0].to_string(), size);
            }
        }
    }
    Ok(sizes)
}

/// Blob contents via `git cat-file --batch`: for each requested oid the
/// response is `<oid> <type> <size>\n<content>\n`. `max_content_bytes` is
/// the caller's own running content budget (`ResourceBudget::max_total_bytes`)
/// — the requested oids were already chosen to fit under it, so the stdout
/// cap here is that budget plus per-oid header headroom, not an independent
/// number: it exists to bound a *misbehaving* git (or an object that lied
/// about its size to `--batch-check`), not to re-enforce the budget itself.
fn blob_contents(
    root: &std::path::Path,
    oids: &[String],
    max_content_bytes: usize,
) -> Result<std::collections::HashMap<String, Vec<u8>>, GitError> {
    let mut contents = std::collections::HashMap::new();
    if oids.is_empty() {
        return Ok(contents);
    }
    let mut input = oids.join("\n");
    input.push('\n');
    let cap = max_content_bytes
        .saturating_add(oids.len().saturating_mul(CAT_FILE_HEADER_HEADROOM_PER_OID))
        .max(DEFAULT_MAX_STDOUT_BYTES);
    let buf = cat_file_stdin(root, "--batch", &input, cap)?;
    let mut pos = 0;
    while pos < buf.len() {
        let nl = buf[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| GitError::GitFailed("truncated cat-file --batch output".to_string()))?;
        let header = String::from_utf8_lossy(&buf[pos..pos + nl]).to_string();
        pos += nl + 1;
        let parts: Vec<&str> = header.split(' ').collect();
        if parts.len() < 3 {
            return Err(GitError::GitFailed(format!("git cat-file: {header}")));
        }
        let size: usize = parts[2]
            .parse()
            .map_err(|_| GitError::GitFailed(format!("git cat-file: {header}")))?;
        if pos + size > buf.len() {
            return Err(GitError::GitFailed(
                "truncated cat-file --batch output".to_string(),
            ));
        }
        contents.insert(parts[0].to_string(), buf[pos..pos + size].to_vec());
        pos += size + 1; // skip content and the trailing newline
    }
    Ok(contents)
}

/// Looks up a fetched blob's content by oid. The all-zero oid (added/deleted
/// side) is the one legitimate case with no fetched content and returns
/// `&[]`; any other oid missing from `contents` is a bug in the fetch/lookup
/// pipeline (or a tampered `cat-file` response) and must fail closed rather
/// than silently rendering as an empty file.
fn blob_of<'a>(
    contents: &'a std::collections::HashMap<String, Vec<u8>>,
    oid: &str,
) -> Result<&'a [u8], GitError> {
    if is_zero_oid(oid) {
        return Ok(&[]);
    }
    contents
        .get(oid)
        .map(|v| v.as_slice())
        .ok_or_else(|| GitError::GitFailed(format!("object {oid} missing from cat-file response")))
}

/// Same heuristic git uses: a NUL byte within the first 8000 bytes.
fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// True if `text` is a Git LFS pointer blob: the reviewer would be looking
/// at a pointer, not the real data, and must be told so.
fn is_lfs_pointer_text(text: &str) -> bool {
    text.starts_with("version https://git-lfs.github.com/spec/v1")
}

/// Checks a file's text sides against the per-file line budgets. Returns the
/// structured warning to emit (and the file degrades to `TooLarge`, which
/// requires an explicit acknowledgement) when a side has too many lines —
/// which also bounds the line-diff CPU spent on it — or a single line too
/// long to render safely.
fn line_budget_violation(
    path: &str,
    old_text: &str,
    new_text: &str,
    budget: &ResourceBudget,
) -> Option<Warning> {
    let line_count = |t: &str| t.bytes().filter(|&b| b == b'\n').count() + 1;
    let count = line_count(old_text).max(line_count(new_text));
    if count > budget.max_file_lines {
        return Some(
            Warning::new(
                "FILE_TOO_MANY_LINES",
                Severity::Warning,
                format!(
                    "file has too many lines to display inline: {path} ({count} lines, limit {})",
                    budget.max_file_lines
                ),
            )
            .with_path(path),
        );
    }
    let longest = old_text
        .split('\n')
        .chain(new_text.split('\n'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    if longest > budget.max_line_bytes {
        return Some(
            Warning::new(
                "LINE_TOO_LONG",
                Severity::Warning,
                format!(
                    "file contains a line too long to display inline: {path} ({longest} bytes, limit {})",
                    budget.max_line_bytes
                ),
            )
            .with_path(path),
        );
    }
    None
}

/// Lowercase wire name of a file type, matching its serde serialization.
fn file_type_name(t: FileType) -> &'static str {
    match t {
        FileType::Regular => "regular",
        FileType::Executable => "executable",
        FileType::Symlink => "symlink",
        FileType::Gitlink => "gitlink",
    }
}

/// Emits per-file warnings for changes the diff body alone under-communicates:
/// mode changes (e.g. the executable bit appearing), file type changes
/// (regular ↔ symlink and the like), gitlink (submodule pointer) changes
/// whose nested diff is not shown, and LFS pointers standing in for real
/// data. These pair with `FileDiff::requires_ack`, which forces an explicit
/// acknowledgement for the same categories.
fn push_shape_warnings(
    warnings: &mut Vec<Warning>,
    entry: &RawEntry,
    old_type: Option<FileType>,
    new_type: Option<FileType>,
    lfs_pointer: bool,
) {
    let path = display_path(entry);
    let (old_mode, new_mode) = (&entry.old_mode, &entry.new_mode);
    let oid_changed = entry.old_oid != entry.new_oid;
    if old_type == Some(FileType::Gitlink) || new_type == Some(FileType::Gitlink) {
        // A same-oid pure rename of a submodule path moves no pointer, so
        // there is nothing hidden to warn about.
        if oid_changed {
            warnings.push(
                Warning::new(
                    "GITLINK_CHANGED",
                    Severity::Warning,
                    format!(
                        "submodule pointer changed: {path} (only the commit pointer is shown; the submodule's own diff is NOT displayed here)"
                    ),
                )
                .with_path(path),
            );
        }
    } else if let (Some(o), Some(n)) = (old_type, new_type) {
        if o != n {
            warnings.push(
                Warning::new(
                    "FILE_TYPE_CHANGED",
                    Severity::Warning,
                    format!(
                        "file type changed: {path} ({} -> {})",
                        file_type_name(o),
                        file_type_name(n)
                    ),
                )
                .with_path(path),
            );
        } else if old_mode != new_mode {
            warnings.push(
                Warning::new(
                    "MODE_CHANGED",
                    Severity::Warning,
                    format!("file mode changed: {path} ({old_mode} -> {new_mode})"),
                )
                .with_path(path),
            );
        }
    }
    if lfs_pointer {
        warnings.push(
            Warning::new(
                "LFS_POINTER",
                Severity::Warning,
                format!(
                    "Git LFS pointer: {path} (the pointer file is shown, NOT the real content)"
                ),
            )
            .with_path(path),
        );
    }
}

/// Text representation of one side (old or new) of a raw entry, used as
/// input to [`text_hunks`]. Never `cat-file`s a gitlink oid (it usually
/// doesn't exist locally as a blob) — a gitlink side becomes the same
/// synthetic `Subproject commit <oid>` line `git diff` prints, so
/// converting a regular file to/from a gitlink still shows the blob content
/// on the non-gitlink side instead of hiding it behind a submodule-only view.
fn side_bytes<'a>(
    mode: &str,
    oid: &str,
    contents: &'a std::collections::HashMap<String, Vec<u8>>,
) -> Result<std::borrow::Cow<'a, [u8]>, GitError> {
    if is_gitlink_mode(mode) && !is_zero_oid(oid) {
        return Ok(std::borrow::Cow::Owned(
            format!("Subproject commit {oid}\n").into_bytes(),
        ));
    }
    blob_of(contents, oid).map(std::borrow::Cow::Borrowed)
}

/// Line-based text diff with 3 lines of context, built directly from blob
/// contents (never from `git diff` porcelain output).
fn text_hunks(old: &str, new: &str) -> Vec<Hunk> {
    use similar::ChangeTag;
    let diff = similar::TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_count = (last.old_range().end - first.old_range().start) as u32;
        let new_count = (last.new_range().end - first.new_range().start) as u32;
        // Unified-diff convention: a zero-count side reports the line
        // *before* the change (0 when at the top of the file).
        let old_start = first.old_range().start as u32 + u32::from(old_count > 0);
        let new_start = first.new_range().start as u32 + u32::from(new_count > 0);
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let (kind, old_no, new_no) = match change.tag() {
                    ChangeTag::Equal => (
                        LineKind::Context,
                        change.old_index().map(|i| i as u32 + 1),
                        change.new_index().map(|i| i as u32 + 1),
                    ),
                    ChangeTag::Delete => (
                        LineKind::Remove,
                        change.old_index().map(|i| i as u32 + 1),
                        None,
                    ),
                    ChangeTag::Insert => (
                        LineKind::Add,
                        None,
                        change.new_index().map(|i| i as u32 + 1),
                    ),
                };
                let value = change.value();
                let (content, eol) = match value.strip_suffix('\n') {
                    Some(rest) => match rest.strip_suffix('\r') {
                        Some(rest) => (rest, Eol::Crlf),
                        None => (rest, Eol::Lf),
                    },
                    None => (value, Eol::None),
                };
                lines.push(DiffLine {
                    kind,
                    content: content.to_string(),
                    eol,
                    old_no,
                    new_no,
                });
            }
        }
        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            section: String::new(),
            lines,
        });
    }
    hunks
}

/// Computes the diff of `<base>...HEAD` (merge-base semantics) directly
/// from blob contents instead of parsing `git diff` text output.
///
/// This is deliberate: the reviewed agent shares the working environment,
/// and `git diff` porcelain output can be manipulated from inside the repo
/// via `.gitattributes` (`-diff` makes files render as "Binary files
/// differ"), textconv/external diff drivers (arbitrary fake content), and
/// config like `diff.noprefix` (breaks `a/`/`b/` parsing). This pipeline
/// only uses plumbing that reads object data:
///
/// 1. `rev-parse --verify` both endpoints (bad base -> `BadBase`),
/// 2. `merge-base` to reproduce `...` semantics,
/// 3. `diff-tree -r -z -M --full-index --raw --ignore-submodules=none` for
///    the file list (NUL delimited, so paths arrive verbatim regardless of
///    `core.quotepath`; `--ignore-submodules=none` overrides
///    `submodule.<name>.ignore` / `.gitmodules` config that would otherwise
///    suppress gitlink change entries),
/// 4. `cat-file --batch-check` / `--batch` for blob sizes and contents,
/// 5. an in-process line diff (`similar`) with 3 context lines.
///
/// None of these read `.gitattributes`, diff drivers, or diff config, so
/// what the reviewer sees is derived from the actual committed blobs.
pub fn compute_diff(root: &std::path::Path, base: &str) -> Result<DiffOutput, GitError> {
    compute_diff_with_budget(root, base, &ResourceBudget::default())
}

/// [`compute_diff`] with an explicit [`ResourceBudget`] (the default budget
/// in production; tests pass tighter ones to exercise the limits).
pub fn compute_diff_with_budget(
    root: &std::path::Path,
    base: &str,
    budget: &ResourceBudget,
) -> Result<DiffOutput, GitError> {
    let base_oid = rev_parse_commit(root, base)?;
    // HEAD is never the user-supplied base, so a resolution failure here is
    // always an internal git problem, not a "bad base ref" report.
    let head_oid = rev_parse_commit(root, "HEAD").map_err(|e| match e {
        GitError::BadBase(msg) => GitError::GitFailed(msg),
        other => other,
    })?;

    let out = run_git(
        root,
        &["merge-base", &base_oid, &head_oid],
        DEFAULT_MAX_STDOUT_BYTES,
    )?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(GitError::GitFailed(format!(
            "no merge base between {base} and HEAD{}{stderr}",
            if stderr.is_empty() { "" } else { ": " }
        )));
    }
    let merge_base = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if merge_base.is_empty() {
        return Err(GitError::GitFailed(format!(
            "no merge base between {base} and HEAD"
        )));
    }

    let out = run_git(
        root,
        &[
            "diff-tree",
            "-r",
            "-z",
            "-M",
            "--full-index",
            "--raw",
            "--ignore-submodules=none",
            &merge_base,
            &head_oid,
        ],
        DIFF_TREE_RAW_MAX_STDOUT_BYTES,
    )?;
    if !out.status.success() {
        return Err(GitError::GitFailed(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    // Bounded parse: bails at the `max_files + 1`th record instead of
    // parsing/allocating the entire raw stream and checking the count only
    // afterward (see `parse_raw_z_capped`). `DIFF_TREE_RAW_MAX_STDOUT_BYTES`
    // already bounds the raw byte buffer (Task 3.1); this bounds the parsed
    // `RawEntry` structure count and the work spent building it.
    let entries = parse_raw_z_capped(&out.stdout, budget.max_files)?;

    // Sizes first (--batch-check), so oversized blobs are never ingested.
    // Gitlink sides are excluded: their oid is a commit in a submodule repo,
    // not a blob in this one, and usually doesn't exist locally at all.
    // Equal-oid entries (pure rename / mode-only change) are still included
    // here so their size can be exposed in `FileDiff`, even though they get
    // `Plan::NoContent` below (--batch-check is cheap: it never reads blob
    // content).
    let mut size_oids: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in &entries {
        for (mode, oid) in [
            (&entry.old_mode, &entry.old_oid),
            (&entry.new_mode, &entry.new_oid),
        ] {
            if !is_zero_oid(oid) && !is_gitlink_mode(mode) && seen.insert(oid.clone()) {
                size_oids.push(oid.clone());
            }
        }
    }
    let sizes = blob_sizes(root, &size_oids)?;

    let mut warnings = Vec::new();
    let mut plans = Vec::with_capacity(entries.len());
    let mut need_oids: Vec<String> = Vec::new();
    let mut need_seen = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;
    for entry in &entries {
        if entry.old_oid == entry.new_oid {
            // Pure rename (R100), mode-only change, or a gitlink pointing at
            // the same commit on both sides: nothing to show. Equal oids mean
            // the blob bytes are byte-for-byte identical, so there is no
            // content diff to display and nothing to hide in it either —
            // this holds regardless of whether the blob happens to be
            // binary or non-UTF-8. `content_kind` is therefore set to `Text`
            // below ("no opaque content change") rather than inspecting the
            // blob to classify it as Binary/NonUtf8, and the review-gate ack
            // requirement (only opaque `content_kind`s need acking) does not
            // apply to this file.
            plans.push(Plan::NoContent);
            continue;
        }
        let mut file_bytes = 0usize;
        let mut max_blob = 0usize;
        for (mode, oid) in [
            (&entry.old_mode, &entry.old_oid),
            (&entry.new_mode, &entry.new_oid),
        ] {
            if is_zero_oid(oid) || is_gitlink_mode(mode) {
                continue;
            }
            let Some(&size) = sizes.get(oid.as_str()) else {
                return Err(GitError::GitFailed(format!(
                    "object {oid} missing (file {})",
                    display_path(entry)
                )));
            };
            file_bytes += size;
            max_blob = max_blob.max(size);
        }
        if max_blob > budget.max_file_bytes {
            warnings.push(
                Warning::new(
                    "FILE_TOO_LARGE",
                    Severity::Warning,
                    format!(
                        "file too large to display inline: {} ({max_blob} bytes)",
                        display_path(entry)
                    ),
                )
                .with_path(display_path(entry)),
            );
            plans.push(Plan::TooLarge);
            continue;
        }
        if total_bytes + file_bytes > budget.max_total_bytes {
            warnings.push(
                Warning::new(
                    "DIFF_TOO_LARGE",
                    Severity::Warning,
                    format!("diff too large: {} not displayed", display_path(entry)),
                )
                .with_path(display_path(entry)),
            );
            plans.push(Plan::TooLarge);
            continue;
        }
        total_bytes += file_bytes;
        for (mode, oid) in [
            (&entry.old_mode, &entry.old_oid),
            (&entry.new_mode, &entry.new_oid),
        ] {
            if !is_zero_oid(oid) && !is_gitlink_mode(mode) && need_seen.insert(oid.clone()) {
                need_oids.push(oid.clone());
            }
        }
        plans.push(Plan::Content);
    }

    let contents = blob_contents(root, &need_oids, budget.max_total_bytes)?;

    let mut files = Vec::with_capacity(entries.len());
    let mut total_lines: usize = 0;
    for (entry, plan) in entries.iter().zip(&plans) {
        let (old_path, new_path) = entry_paths(entry);
        let mut lfs_pointer = false;
        let (content_kind, hunks) = match plan {
            Plan::NoContent => (ContentKind::Text, Vec::new()),
            Plan::TooLarge => (ContentKind::TooLarge, Vec::new()),
            Plan::Content => {
                let old = side_bytes(&entry.old_mode, &entry.old_oid, &contents)?;
                let new = side_bytes(&entry.new_mode, &entry.new_oid, &contents)?;
                if is_binary(&old) || is_binary(&new) {
                    (ContentKind::Binary, Vec::new())
                } else {
                    match (std::str::from_utf8(&old), std::str::from_utf8(&new)) {
                        (Ok(old_text), Ok(new_text)) => {
                            lfs_pointer =
                                is_lfs_pointer_text(old_text) || is_lfs_pointer_text(new_text);
                            if let Some(w) = line_budget_violation(
                                display_path(entry),
                                old_text,
                                new_text,
                                budget,
                            ) {
                                warnings.push(w);
                                (ContentKind::TooLarge, Vec::new())
                            } else {
                                (ContentKind::Text, text_hunks(old_text, new_text))
                            }
                        }
                        _ => {
                            // Different byte contents can lossy-decode to the
                            // same string; never diff lossily.
                            warnings.push(
                                Warning::new(
                                    "NON_UTF8_CONTENT",
                                    Severity::Warning,
                                    format!(
                                        "file content is not valid UTF-8, not rendered: {}",
                                        display_path(entry)
                                    ),
                                )
                                .with_path(display_path(entry)),
                            );
                            (ContentKind::NonUtf8, Vec::new())
                        }
                    }
                }
            }
        };
        // Whole-review line budget: a file whose rendered lines would push
        // the total past the cap degrades to an explicitly-acknowledged
        // TooLarge card instead of being silently truncated (bounds the
        // session JSON and the DOM the browser has to build).
        let file_lines: usize = hunks.iter().map(|h| h.lines.len()).sum();
        let (content_kind, hunks) = if total_lines + file_lines > budget.max_total_lines {
            warnings.push(
                Warning::new(
                    "DIFF_TOO_LARGE",
                    Severity::Warning,
                    format!(
                        "total diff line budget ({}) exceeded: {} not displayed",
                        budget.max_total_lines,
                        display_path(entry)
                    ),
                )
                .with_path(display_path(entry)),
            );
            (ContentKind::TooLarge, Vec::new())
        } else {
            total_lines += file_lines;
            (content_kind, hunks)
        };
        let old_type = file_type_of_mode(&entry.old_mode);
        let new_type = file_type_of_mode(&entry.new_mode);
        push_shape_warnings(&mut warnings, entry, old_type, new_type, lfs_pointer);
        files.push(FileDiff {
            old_path,
            new_path,
            change_kind: entry_change_kind(entry),
            content_kind,
            old_mode: mode_opt(&entry.old_mode),
            new_mode: mode_opt(&entry.new_mode),
            old_type,
            new_type,
            old_oid: oid_opt(&entry.old_oid),
            new_oid: oid_opt(&entry.new_oid),
            old_size: blob_size(&sizes, &entry.old_mode, &entry.old_oid),
            new_size: blob_size(&sizes, &entry.new_mode, &entry.new_oid),
            lfs_pointer,
            hunks,
        });
    }

    Ok(DiffOutput {
        files,
        warnings,
        base_oid,
        head_oid,
        merge_base_oid: merge_base,
    })
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let st = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn fixture_repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        std::fs::write(d.join("b.txt"), "new file\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);
        td
    }

    /// Repo with just a base commit on `main` and a `feature` branch
    /// checked out, ready for per-test changes.
    fn base_repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        td
    }

    fn find<'a>(files: &'a [FileDiff], new_path: &str) -> &'a FileDiff {
        files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(new_path))
            .unwrap_or_else(|| panic!("no file with new_path {new_path}"))
    }

    fn hunk_contents(f: &FileDiff) -> Vec<&str> {
        f.hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect()
    }

    /// Runs git in `dir` and returns trimmed stdout (asserting success).
    fn git_stdout(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn replace_refs_cannot_hide_changes() {
        // An agent can `git replace` the real HEAD with a fake commit whose
        // tree equals the base tree; every git command then silently reads
        // the fake commit and the diff comes back empty. --no-replace-objects
        // must defeat this.
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nEVIL\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "evil change"]);
        let head = git_stdout(d, &["rev-parse", "HEAD"]);
        let base_tree = git_stdout(d, &["rev-parse", "main^{tree}"]);
        let fake = git_stdout(
            d,
            &["commit-tree", &base_tree, "-p", "main", "-m", "innocent"],
        );
        git(d, &["replace", &head, &fake]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.change_kind, ChangeKind::Modified);
        assert!(hunk_contents(a).contains(&"EVIL"));
    }

    #[test]
    fn grafts_cannot_move_the_merge_base() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nEVIL\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "evil change"]);
        let head = git_stdout(d, &["rev-parse", "HEAD"]);
        let main_oid = git_stdout(d, &["rev-parse", "main"]);
        // Graft main's tip to claim HEAD as its parent: HEAD becomes an ancestor
        // of main, so merge-base(main, HEAD) = HEAD and the diff collapses to
        // empty — unless grafts are disabled.
        let graft_dir = d.join(".git").join("info");
        std::fs::create_dir_all(&graft_dir).unwrap();
        std::fs::write(graft_dir.join("grafts"), format!("{main_oid} {head}\n")).unwrap();

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert!(hunk_contents(a).contains(&"EVIL"));
    }

    #[test]
    fn computes_diff_against_base() {
        let td = fixture_repo();
        let out = compute_diff(td.path(), "main").unwrap();
        assert!(out.warnings.is_empty());
        assert_eq!(out.files.len(), 2);
        let a = find(&out.files, "a.txt");
        assert_eq!(a.change_kind, ChangeKind::Modified);
        let h = &a.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 3, 1, 4)
        );
        let contents = hunk_contents(a);
        assert!(contents.contains(&"TWO"));
        assert!(contents.contains(&"four"));
        // line numbering: "one" is context 1/1, "two" removed at old 2,
        // "TWO" added at new 2.
        assert_eq!(h.lines[0].old_no, Some(1));
        assert_eq!(h.lines[0].new_no, Some(1));
        let removed = h
            .lines
            .iter()
            .find(|l| matches!(l.kind, LineKind::Remove))
            .unwrap();
        assert_eq!((removed.old_no, removed.new_no), (Some(2), None));
        let b = find(&out.files, "b.txt");
        assert_eq!(b.change_kind, ChangeKind::Added);
        assert_eq!(b.old_path, None);
        assert_eq!(b.hunks[0].old_start, 0);
        assert_eq!(b.hunks[0].old_count, 0);
        assert_eq!(b.hunks[0].new_start, 1);
        assert_eq!(b.hunks[0].new_count, 1);
    }

    #[test]
    fn bad_base_is_distinguished() {
        let td = fixture_repo();
        assert!(matches!(
            compute_diff(td.path(), "no-such-ref"),
            Err(GitError::BadBase(_))
        ));
    }

    #[test]
    fn empty_diff_returns_empty_vec() {
        let td = fixture_repo();
        let out = compute_diff(td.path(), "HEAD").unwrap();
        assert!(out.files.is_empty());
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn current_branch_name() {
        let td = fixture_repo();
        assert_eq!(current_branch(td.path()), "feature");
    }

    #[test]
    fn non_ascii_path_is_verbatim() {
        // The raw entries are NUL-delimited (`diff-tree -z`), so non-ASCII
        // paths must arrive verbatim regardless of `core.quotepath`.
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("日本語.txt"), "hello\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "add japanese file"]);

        let out = compute_diff(d, "main").unwrap();
        let f = out
            .files
            .iter()
            .find(|f| f.change_kind == ChangeKind::Added)
            .expect("added file present");
        assert_eq!(f.new_path.as_deref(), Some("日本語.txt"));
    }

    #[test]
    fn gitattributes_no_diff_cannot_hide_content() {
        // An agent marking files `-diff` via a committed .gitattributes
        // must not turn text changes into "Binary files differ".
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join(".gitattributes"), "*.txt -diff\n").unwrap();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "sneaky"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.change_kind, ChangeKind::Modified);
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn worktree_gitattributes_cannot_hide_content() {
        // Same attack via an uncommitted .gitattributes in the working tree
        // (git's diff machinery reads attributes from the worktree).
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);
        std::fs::write(d.join(".gitattributes"), "*.txt -diff\n").unwrap();

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.change_kind, ChangeKind::Modified);
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn textconv_driver_cannot_fake_content() {
        // A repo-local textconv driver would let `git diff` display
        // arbitrary fake content; the blob-based diff must show the real
        // committed bytes.
        let td = base_repo();
        let d = td.path();
        git(d, &["config", "diff.x.textconv", "printf FAKE #"]);
        std::fs::write(d.join(".gitattributes"), "*.txt diff=x\n").unwrap();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "sneaky"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        let contents = hunk_contents(a);
        assert!(contents.contains(&"TWO"));
        assert!(!contents.iter().any(|c| c.contains("FAKE")));
    }

    #[test]
    fn external_diff_env_var_has_no_effect() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        // Would make `git diff` invoke a bogus external command; compute_diff
        // strips it (and uses plumbing that ignores it anyway).
        std::env::set_var("GIT_EXTERNAL_DIFF", "/nonexistent/fake-diff");
        let result = compute_diff(d, "main");
        std::env::remove_var("GIT_EXTERNAL_DIFF");
        let out = result.unwrap();
        let a = find(&out.files, "a.txt");
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn noprefix_config_cannot_break_paths() {
        // `diff.noprefix=true` breaks parsers expecting `a/`/`b/` prefixes;
        // the raw pipeline never sees prefixes at all.
        let td = base_repo();
        let d = td.path();
        git(d, &["config", "diff.noprefix", "true"]);
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.old_path.as_deref(), Some("a.txt"));
        assert_eq!(a.new_path.as_deref(), Some("a.txt"));
        assert!(!a.hunks.is_empty());
    }

    #[test]
    fn pure_rename_has_no_hunks() {
        let td = base_repo();
        let d = td.path();
        git(d, &["mv", "a.txt", "renamed.txt"]);
        git(d, &["commit", "-m", "rename"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        let f = &out.files[0];
        assert_eq!(f.change_kind, ChangeKind::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("renamed.txt"));
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn rename_with_modification_has_hunks() {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(d.join("a.txt"), &body).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        git(d, &["mv", "a.txt", "moved.txt"]);
        std::fs::write(d.join("moved.txt"), body.replace("line5", "LINE5")).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "rename and edit"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        let f = &out.files[0];
        assert_eq!(f.change_kind, ChangeKind::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("moved.txt"));
        let contents = hunk_contents(f);
        assert!(contents.contains(&"LINE5"));
    }

    #[test]
    fn submodule_ignore_all_config_cannot_hide_gitlink_change() {
        let td = base_repo();
        let d = td.path();
        let sha1 = "1111111111111111111111111111111111111111";
        let sha2 = "2222222222222222222222222222222222222222";
        git(
            d,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha1},sub"),
            ],
        );
        std::fs::write(
            d.join(".gitmodules"),
            "[submodule \"sub\"]\n\tpath = sub\n\turl = ./sub\n\tignore = all\n",
        )
        .unwrap();
        git(d, &["add", ".gitmodules"]);
        git(d, &["commit", "-m", "add submodule"]);
        // Move main forward so the gitlink change is on the feature side only.
        git(d, &["checkout", "main"]);
        git(d, &["merge", "--ff-only", "feature"]);
        git(d, &["checkout", "feature"]);
        git(d, &["config", "submodule.sub.ignore", "all"]);
        git(
            d,
            &["update-index", "--cacheinfo", &format!("160000,{sha2},sub")],
        );
        git(d, &["commit", "-m", "bump submodule"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "sub");
        let contents = hunk_contents(f);
        assert!(
            contents.iter().any(|c| c.contains(sha2)),
            "gitlink bump must be visible: {contents:?}"
        );
    }

    #[test]
    fn submodule_pointer_change_is_visible() {
        let td = base_repo();
        let d = td.path();
        let sha = "1111111111111111111111111111111111111111";
        git(
            d,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},sub"),
            ],
        );
        git(d, &["commit", "-m", "add submodule pointer"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "sub");
        assert_eq!(f.change_kind, ChangeKind::Added);
        assert_eq!(f.hunks.len(), 1);
        let lines = &f.hunks[0].lines;
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0].kind, LineKind::Add));
        assert_eq!(lines[0].content, format!("Subproject commit {sha}"));
        assert_eq!(lines[0].new_no, Some(1));
    }

    #[test]
    fn file_to_gitlink_conversion_shows_removed_content() {
        let td = base_repo();
        let d = td.path();
        // a.txt exists on main with content "one\ntwo\nthree\n". Convert it to a gitlink.
        let sha = "3333333333333333333333333333333333333333";
        git(d, &["rm", "--cached", "a.txt"]);
        std::fs::remove_file(d.join("a.txt")).unwrap();
        git(
            d,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},a.txt"),
            ],
        );
        git(d, &["commit", "-m", "file to gitlink"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "a.txt");
        let contents = hunk_contents(f);
        assert!(
            contents.contains(&"two"),
            "old file content must be visible: {contents:?}"
        );
        assert!(
            contents.iter().any(|c| c.contains(sha)),
            "new gitlink must be visible: {contents:?}"
        );
    }

    #[test]
    fn gitlink_to_file_conversion_shows_added_content() {
        let td = base_repo();
        let d = td.path();
        let sha = "4444444444444444444444444444444444444444";
        git(
            d,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},sub"),
            ],
        );
        git(d, &["commit", "-m", "add gitlink"]);
        git(d, &["checkout", "main"]);
        git(d, &["merge", "--ff-only", "feature"]);
        git(d, &["checkout", "feature"]);
        git(d, &["rm", "--cached", "sub"]);
        // Checking out a gitlink entry leaves an empty directory at its path
        // (the submodule mount point); it must go before `sub` can become a
        // regular file.
        let _ = std::fs::remove_dir(d.join("sub"));
        std::fs::write(d.join("sub"), "EVIL payload\n").unwrap();
        git(d, &["add", "sub"]);
        git(d, &["commit", "-m", "gitlink to file"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "sub");
        let contents = hunk_contents(f);
        assert!(
            contents.contains(&"EVIL payload"),
            "added file content must be visible: {contents:?}"
        );
        assert!(
            contents.iter().any(|c| c.contains(sha)),
            "old gitlink must be visible: {contents:?}"
        );
    }

    #[test]
    fn nul_bytes_are_binary() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("blob.bin"), b"\x00\x01\x02text").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "binary"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "blob.bin");
        assert_eq!(f.content_kind, ContentKind::Binary);
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn non_utf8_blobs_are_not_lossy_diffed() {
        // 0x80 -> 0x81: neither contains NUL, both are invalid UTF-8, and
        // both lossy-decode to "\u{FFFD}\n" — a lossy text diff would claim
        // "no content changes" for a real byte-level change.
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("data.txt"), [0x80, b'\n']).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        std::fs::write(d.join("data.txt"), [0x81, b'\n']).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.files[0].content_kind, ContentKind::NonUtf8);
        assert!(out.files[0].hunks.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].message.contains("data.txt"),
            "warning should name the file: {}",
            out.warnings[0].message
        );
        assert_eq!(out.warnings[0].code, "NON_UTF8_CONTENT");
    }

    #[test]
    fn oversized_file_is_too_large_with_warning() {
        let td = base_repo();
        let d = td.path();
        let big = "x".repeat(MAX_FILE_BYTES + 1);
        std::fs::write(d.join("big.txt"), &big).unwrap();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "big"]);

        let out = compute_diff(d, "main").unwrap();
        let big_file = find(&out.files, "big.txt");
        assert_eq!(big_file.content_kind, ContentKind::TooLarge);
        assert!(big_file.hunks.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0]
                .message
                .contains("file too large to display inline: big.txt"),
            "unexpected warning: {}",
            out.warnings[0].message
        );
        assert_eq!(out.warnings[0].code, "FILE_TOO_LARGE");
        // Other files in the same diff are unaffected.
        let a = find(&out.files, "a.txt");
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn three_dot_semantics_use_merge_base() {
        // Commits on the base branch after the fork point must not appear
        // in the diff (`<base>...HEAD` semantics, not `<base>..HEAD`).
        let td = fixture_repo();
        let d = td.path();
        git(d, &["checkout", "main"]);
        std::fs::write(d.join("main_only.txt"), "main moved on\n").unwrap();
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\nmain edit\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "main advances"]);
        git(d, &["checkout", "feature"]);

        let out = compute_diff(d, "main").unwrap();
        assert!(
            !out.files
                .iter()
                .any(|f| f.new_path.as_deref() == Some("main_only.txt")),
            "base-side commit leaked into the diff"
        );
        // The feature-side change is still reported relative to the fork.
        let a = find(&out.files, "a.txt");
        let contents = hunk_contents(a);
        assert!(contents.contains(&"TWO"));
        assert!(!contents.iter().any(|c| c.contains("main edit")));
    }

    #[test]
    fn mode_only_change_exposes_modes() {
        let td = base_repo();
        let d = td.path();
        git(d, &["update-index", "--chmod=+x", "a.txt"]);
        git(d, &["commit", "-m", "make executable"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.change_kind, ChangeKind::Modified);
        assert_eq!(a.content_kind, ContentKind::Text);
        assert!(a.hunks.is_empty());
        assert_eq!(a.old_mode.as_deref(), Some("100644"));
        assert_eq!(a.new_mode.as_deref(), Some("100755"));
        assert_eq!(a.old_oid, a.new_oid);
        assert!(a.old_oid.is_some());
        assert!(
            a.old_size.is_some(),
            "size must be fetched even for equal-oid entries"
        );
    }

    #[test]
    fn binary_file_exposes_oids_and_sizes() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("blob.bin"), b"\x00\x01\x02text").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "binary"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "blob.bin");
        assert_eq!(f.content_kind, ContentKind::Binary);
        assert_eq!(f.change_kind, ChangeKind::Added);
        assert_eq!(f.old_oid, None);
        assert!(f.new_oid.is_some());
        assert_eq!(f.new_size, Some(7));
        assert_eq!(f.new_mode.as_deref(), Some("100644"));
    }

    #[test]
    fn executable_bit_flip_reports_mode_change_and_requires_ack() {
        let td = base_repo();
        let d = td.path();
        git(d, &["update-index", "--chmod=+x", "a.txt"]);
        git(d, &["commit", "-m", "make executable"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "a.txt");
        assert_eq!(f.old_type, Some(FileType::Regular));
        assert_eq!(f.new_type, Some(FileType::Executable));
        assert!(
            f.requires_ack(),
            "a mode change must require explicit acknowledgement"
        );
        // The executable bit is part of the file *type* taxonomy here, so
        // the flip surfaces as FILE_TYPE_CHANGED (regular -> executable).
        assert!(
            out.warnings.iter().any(|w| w.code == "FILE_TYPE_CHANGED"
                && w.path.as_deref() == Some("a.txt")
                && w.message.contains("regular -> executable")),
            "missing FILE_TYPE_CHANGED warning: {:?}",
            out.warnings
        );
    }

    #[cfg(unix)]
    #[test]
    fn regular_to_symlink_reports_type_change() {
        let td = base_repo();
        let d = td.path();
        std::fs::remove_file(d.join("a.txt")).unwrap();
        std::os::unix::fs::symlink("target.txt", d.join("a.txt")).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "symlinkify"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "a.txt");
        assert_eq!(f.old_type, Some(FileType::Regular));
        assert_eq!(f.new_type, Some(FileType::Symlink));
        assert!(f.requires_ack());
        assert!(
            out.warnings.iter().any(|w| w.code == "FILE_TYPE_CHANGED"),
            "missing FILE_TYPE_CHANGED warning: {:?}",
            out.warnings
        );
    }

    #[test]
    fn crlf_and_missing_final_newline_are_visible_in_eol() {
        let td = base_repo();
        let d = td.path();
        // LF -> CRLF on line 1; the last line loses its final newline. The
        // display content strips line endings, so without `eol` these
        // would render as identical-looking strings.
        std::fs::write(d.join("eol.txt"), "alpha\nlast\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base eol"]);
        std::fs::write(d.join("eol.txt"), "alpha\r\nlast").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change eol"]);

        // Both commits are on `feature`, so diff the last commit itself.
        let out = compute_diff(d, "HEAD~1").unwrap();
        let f = find(&out.files, "eol.txt");
        let lines: Vec<&DiffLine> = f.hunks.iter().flat_map(|h| &h.lines).collect();
        let eol_of = |kind: LineKind, content: &str| {
            lines
                .iter()
                .find(|l| {
                    matches!(
                        (&l.kind, kind),
                        (LineKind::Add, LineKind::Add)
                            | (LineKind::Remove, LineKind::Remove)
                            | (LineKind::Context, LineKind::Context)
                    ) && l.content == content
                })
                .unwrap_or_else(|| panic!("no {kind:?} line with content {content:?}: {lines:?}"))
                .eol
        };
        assert_eq!(eol_of(LineKind::Remove, "alpha"), Eol::Lf);
        assert_eq!(eol_of(LineKind::Add, "alpha"), Eol::Crlf);
        assert_eq!(eol_of(LineKind::Remove, "last"), Eol::Lf);
        assert_eq!(eol_of(LineKind::Add, "last"), Eol::None);
    }

    #[test]
    fn lfs_pointer_is_flagged() {
        let td = base_repo();
        let d = td.path();
        let pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 12345\n";
        std::fs::write(d.join("model.bin"), pointer).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "lfs pointer"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "model.bin");
        assert!(f.lfs_pointer, "pointer blob must be flagged as LFS");
        assert!(
            out.warnings.iter().any(|w| w.code == "LFS_POINTER"),
            "missing LFS_POINTER warning: {:?}",
            out.warnings
        );
    }

    #[test]
    fn too_many_files_refuses_with_budget_error() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("one.txt"), "1\n").unwrap();
        std::fs::write(d.join("two.txt"), "2\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "two files"]);

        let budget = ResourceBudget {
            max_files: 1,
            ..ResourceBudget::default()
        };
        match compute_diff_with_budget(d, "main", &budget) {
            Err(GitError::BudgetExceeded(msg)) => {
                // The early-exit parse bails at the (max_files + 1)th
                // record without ever discovering the true total, so the
                // message reports the limit rather than an exact count.
                // Assert the exact boundary substring rather than a bare
                // '1' (which would also match stray digits anywhere in
                // the message and not actually verify the limit).
                assert!(
                    msg.contains("more than 1 files"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn per_file_line_budget_degrades_to_acknowledged_too_large() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("many.txt"), "1\n2\n3\n4\n5\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "many lines"]);

        let budget = ResourceBudget {
            max_file_lines: 3,
            ..ResourceBudget::default()
        };
        let out = compute_diff_with_budget(d, "main", &budget).unwrap();
        let f = find(&out.files, "many.txt");
        assert_eq!(f.content_kind, ContentKind::TooLarge);
        assert!(f.hunks.is_empty());
        assert!(f.requires_ack(), "degraded file must require an ack");
        assert!(out.warnings.iter().any(|w| w.code == "FILE_TOO_MANY_LINES"));
    }

    #[test]
    fn total_line_budget_degrades_later_files_not_silently() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a_first.txt"), "1\n2\n").unwrap();
        std::fs::write(d.join("b_second.txt"), "1\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "two files"]);

        let budget = ResourceBudget {
            max_total_lines: 2,
            ..ResourceBudget::default()
        };
        let out = compute_diff_with_budget(d, "main", &budget).unwrap();
        let first = find(&out.files, "a_first.txt");
        assert_eq!(first.content_kind, ContentKind::Text);
        assert!(!first.hunks.is_empty());
        let second = find(&out.files, "b_second.txt");
        assert_eq!(second.content_kind, ContentKind::TooLarge);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.code == "DIFF_TOO_LARGE" && w.path.as_deref() == Some("b_second.txt")),
            "the dropped file must be named in a structured warning: {:?}",
            out.warnings
        );
    }

    #[cfg(unix)]
    #[test]
    fn wedged_subprocess_is_killed_at_the_deadline() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("60")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let start = std::time::Instant::now();
        // Spawned via `spawn_grouped` (not a plain `cmd.spawn()`): the
        // process-group kill in `wait_with_timeout` needs the child to be
        // its own group leader, exactly like every real call site sets up
        // via `timed_output_with_deadline` / `cat_file_stdin`.
        let child = spawn_grouped(&mut cmd).unwrap();
        let err = wait_with_timeout(
            child,
            std::time::Duration::from_millis(100),
            DEFAULT_MAX_STDOUT_BYTES,
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "kill took too long: {:?}",
            start.elapsed()
        );
    }

    /// The core P0-6 regression: a direct child that backgrounds a
    /// descendant and exits, leaving the descendant holding stdout/stderr
    /// open, must not wedge `wait_with_timeout` past its deadline. The old
    /// implementation unconditionally `.join()`ed the reader threads after
    /// `try_wait` saw the direct child exit — since the descendant (not the
    /// direct child) still holds the pipe open, that join never returned.
    /// Process-group kill is what fixes it: `sh` is spawned via
    /// `spawn_grouped` into its own group, `sleep 5 &` inherits that group,
    /// and once the deadline is hit the whole group is killed, not just the
    /// (already-exited) direct child.
    #[cfg(unix)]
    #[test]
    fn descendant_holding_pipe_does_not_wedge_past_deadline() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 5 & exit 0")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let start = std::time::Instant::now();
        let child = spawn_grouped(&mut cmd).unwrap();
        let result = wait_with_timeout(
            child,
            std::time::Duration::from_millis(200),
            DEFAULT_MAX_STDOUT_BYTES,
        );
        let elapsed = start.elapsed();
        assert!(
            result.is_err(),
            "a descendant holding the pipe open must not be mistaken for a clean exit, got {result:?}"
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "must return by ~deadline instead of wedging on the descendant: {elapsed:?}"
        );
    }

    /// The regression test for the `cat_file_stdin` writer-join bug:
    /// `join_writer_bounded` must not block past `grace` even when the
    /// writer thread genuinely never finishes. In production this happens
    /// when a tampered `git`'s direct child exits 0 (so `wait_with_timeout`
    /// legitimately returns `Ok` — stdout/stderr both EOF'd) while a
    /// descendant it spawned keeps the stdin *read* end open without ever
    /// reading it; a `--batch` request for a large diff is easily past the
    /// ~64 KiB pipe buffer (see the doc comment on `join_writer_bounded`),
    /// so `write_all` blocks forever on the old unconditional `.join()`.
    ///
    /// This constructs that "full pipe, nothing draining it" condition
    /// directly with a raw `libc::pipe`, rather than via a shell descendant.
    /// An earlier version of this test used `sh`/`sleep` to play the
    /// tampered-git role, mirroring the existing `wait_with_timeout`
    /// regression tests' style — but that turned out to be fragile in
    /// practice for the write side specifically: getting a *descendant*
    /// process to reliably retain a duplicated read end of the stdin pipe,
    /// across dash vs bash, job-control, and `spawn_grouped`'s
    /// `process_group(0)`, proved to not generalize the way the
    /// already-existing reader-side tests do (a `dash`-as-`/bin/sh` fixture
    /// that verified correct in isolation still failed the same way inside
    /// the actual `Command`/`spawn_grouped` harness). A raw pipe removes
    /// all of that: `read_fd` is simply never read from and never closed
    /// until this test says so, so the writer is guaranteed to block once
    /// the kernel pipe buffer (~64 KiB) fills, on every platform, with no
    /// shell or subprocess involved at all.
    ///
    /// The whole check still runs on a background thread with a generous
    /// `recv_timeout` on the main thread: if `join_writer_bounded`
    /// regresses back to an unbounded block, this test fails promptly
    /// instead of hanging the entire suite.
    #[cfg(unix)]
    #[test]
    fn cat_file_stdin_writer_does_not_block_forever_on_stuck_reader() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                use std::os::unix::io::FromRawFd;

                let mut fds = [0 as libc::c_int; 2];
                // SAFETY: `fds` is a valid, correctly-sized out-pointer for
                // `pipe(2)`; the call either fills both slots with fresh
                // fds or returns -1 (checked immediately below).
                assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
                let read_fd = fds[0];
                let write_fd = fds[1];
                // SAFETY: `write_fd` was just returned by `pipe(2)` above,
                // is open, and is not owned by anything else yet.
                let mut w = unsafe { std::fs::File::from_raw_fd(write_fd) };

                // Comfortably larger than any real pipe buffer (typically
                // 16-64 KiB) so `write_all` is guaranteed to block once the
                // kernel buffer fills, since `read_fd` is deliberately never
                // read from below.
                let payload = vec![b'x'; 8 * 1024 * 1024];
                let writer = std::thread::spawn(move || -> std::io::Result<()> {
                    use std::io::Write;
                    w.write_all(&payload)
                });

                let start = std::time::Instant::now();
                let result = join_writer_bounded(writer, KILL_GRACE);
                let elapsed = start.elapsed();
                assert!(
                    result.is_err(),
                    "a writer stuck on a full pipe (nothing draining it) must not be \
                     mistaken for a completed write: {result:?}"
                );
                assert!(
                    elapsed < KILL_GRACE + std::time::Duration::from_secs(3),
                    "join_writer_bounded must return by ~KILL_GRACE instead of blocking \
                     forever on the stuck writer: {elapsed:?}"
                );

                // `join_writer_bounded` already detached the writer thread
                // (it never finished within `grace`); closing `read_fd` now
                // makes its blocked `write_all` fail with EPIPE so that
                // leaked thread unwinds instead of staying blocked for the
                // life of the test process.
                // SAFETY: `read_fd` is still open (never touched since the
                // `pipe(2)` call above) and not aliased by anything else.
                unsafe { libc::close(read_fd) };
            });
            let _ = tx.send(result);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(Ok(())) => {}
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(_) => panic!(
                "join_writer_bounded did not return within 30s: it has regressed to an \
                 unbounded block"
            ),
        }
    }

    /// A hostile or misbehaving git that streams unbounded stdout must be
    /// caught at the cap — not buffered without limit, and not only
    /// discovered at the wall-clock deadline. `cat /dev/zero` never
    /// produces EOF on its own, so a passing test here proves the cap is
    /// what stops the read, not the (much longer) timeout.
    #[cfg(unix)]
    #[test]
    fn stdout_overflow_is_bounded_and_kills() {
        let mut cmd = std::process::Command::new("cat");
        cmd.arg("/dev/zero")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let cap = 64 * 1024;
        let start = std::time::Instant::now();
        let child = spawn_grouped(&mut cmd).unwrap();
        let err = wait_with_timeout(child, std::time::Duration::from_secs(5), cap).unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge);
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "an overflowing stdout must be caught promptly at the cap, not only at the 5s deadline: {elapsed:?}"
        );
    }

    /// stderr is a ring buffer, not a truncating `Vec`: when a command
    /// emits more than [`STDERR_CAP`], the *tail* (most recent bytes) must
    /// survive, since that's where git actually puts its error message.
    #[cfg(unix)]
    #[test]
    fn stderr_overflow_keeps_the_tail_not_the_head() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(r#"awk 'BEGIN{for(i=0;i<3000;i++) printf "LINE-%05d\n", i}' 1>&2; exit 1"#);
        let output = timed_output_with_deadline(
            cmd,
            std::time::Duration::from_secs(10),
            DEFAULT_MAX_STDOUT_BYTES,
        )
        .unwrap();
        assert!(!output.status.success(), "the script exits 1");
        assert!(
            output.stderr.len() <= STDERR_CAP,
            "stderr must be bounded at {STDERR_CAP} bytes, got {}",
            output.stderr.len()
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("LINE-00000"),
            "the head of a >cap stderr must have been evicted from the ring buffer: {stderr:?}"
        );
        assert!(
            stderr.contains("LINE-02999"),
            "the tail of stderr (where git's actual error is) must be kept: {stderr:?}"
        );
    }

    /// `rev_parse_commit_with_deadline` must honor whatever timeout its
    /// caller passes rather than always falling back to the 60s
    /// [`GIT_TIMEOUT`] default — that's the whole point of adding it:
    /// `check_head_freshness` in `server.rs` needs a 10s deadline so a
    /// wedged rev-parse can't hold the submit past the server's own 30s
    /// request timeout, let alone the client's 40s.
    ///
    /// Spawning a genuinely wedged `git` process would mean swapping `git`
    /// out on `PATH` for the duration of the test, which is a process-wide
    /// mutation that would race every other test in this module that shells
    /// out to a real `git` concurrently. Instead this exercises
    /// `timed_output_with_deadline` — the exact spawn-with-piped-io-then-wait
    /// helper `rev_parse_commit_with_deadline` calls with its `timeout`
    /// argument — the same way `wedged_subprocess_is_killed_at_the_deadline`
    /// above substitutes `sleep` for `wait_with_timeout` directly.
    #[cfg(unix)]
    #[test]
    fn rev_parse_honors_short_deadline() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("60");
        let start = std::time::Instant::now();
        let err = timed_output_with_deadline(
            cmd,
            std::time::Duration::from_millis(100),
            DEFAULT_MAX_STDOUT_BYTES,
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "kill took too long: {:?}",
            start.elapsed()
        );
    }

    /// The 60s diff-computation path must keep its generous default: passing
    /// a real, fast rev-parse through `rev_parse_commit` (which delegates to
    /// `rev_parse_commit_with_deadline(.., GIT_TIMEOUT)`) still succeeds —
    /// adding the deadline parameter must not have narrowed the default.
    #[test]
    fn rev_parse_commit_default_still_resolves_head() {
        let td = base_repo();
        let head = rev_parse_commit(td.path(), "HEAD").unwrap();
        assert_eq!(head.len(), 40, "expected a full oid, got {head:?}");
    }

    #[test]
    fn worktree_status_reports_tracked_untracked_and_clean() {
        let td = fixture_repo();
        let d = td.path();
        assert!(worktree_status(d).unwrap().is_clean());

        std::fs::write(d.join("a.txt"), "modified\n").unwrap();
        std::fs::write(d.join("brand-new.rs"), "fn main() {}\n").unwrap();
        let status = worktree_status(d).unwrap();
        assert_eq!(status.tracked_changes, vec!["a.txt".to_string()]);
        assert_eq!(status.untracked, vec!["brand-new.rs".to_string()]);
        assert!(status.submodules_dirty.is_empty());
    }

    #[test]
    fn parse_status_v2_z_handles_rename_and_rejects_garbage() {
        // A rename entry's original path is a separate NUL token and must be
        // consumed with its entry, not misread as another record.
        let bytes = b"2 R. N... 100644 100644 100644 1111111111111111111111111111111111111111 1111111111111111111111111111111111111111 R100 new.txt\0old.txt\0? loose.txt\0".to_vec();
        let status = parse_status_v2_z(&bytes).unwrap();
        assert_eq!(status.tracked_changes, vec!["new.txt".to_string()]);
        assert_eq!(status.untracked, vec!["loose.txt".to_string()]);

        // A submodule with inner modifications is its own category.
        let bytes = b"1 .M S.M. 160000 160000 160000 1111111111111111111111111111111111111111 1111111111111111111111111111111111111111 sub\0".to_vec();
        let status = parse_status_v2_z(&bytes).unwrap();
        assert_eq!(status.submodules_dirty, vec!["sub".to_string()]);

        // A submodule whose only change is the commit pointer is an
        // ordinary (committable) tracked change.
        let bytes = b"1 .M SC.. 160000 160000 160000 1111111111111111111111111111111111111111 1111111111111111111111111111111111111111 sub\0".to_vec();
        let status = parse_status_v2_z(&bytes).unwrap();
        assert_eq!(status.tracked_changes, vec!["sub".to_string()]);
        assert!(status.submodules_dirty.is_empty());

        // Fail closed on an entry shape this parser doesn't understand.
        assert!(parse_status_v2_z(b"x whatever\0").is_err());
        assert!(parse_status_v2_z(b"1 .M\0").is_err());
    }

    /// Builds a synthetic `git status --porcelain=v2 -z` buffer with `n`
    /// distinct untracked (`?`) entries.
    fn untracked_entries(n: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        for i in 0..n {
            buf.extend_from_slice(format!("? file{i}.txt\0").as_bytes());
        }
        buf
    }

    #[test]
    fn status_exactly_at_cap_is_not_overflowed() {
        // Boundary: exactly STATUS_MAX_ENTRIES entries must be enumerated
        // in full, not treated as an overflow (avoids a false positive on
        // a merely very messy, but still enumerable, worktree).
        let bytes = untracked_entries(STATUS_MAX_ENTRIES);
        let status = parse_status_v2_z(&bytes).unwrap();
        assert!(status.overflow.is_none());
        assert_eq!(status.untracked.len(), STATUS_MAX_ENTRIES);
        assert!(!status.is_clean());
    }

    #[test]
    fn status_overflow_blocks_as_dirty() {
        // One entry past the cap: parsing must stop enumerating paths and
        // report the worktree as dirty via a summary, never as clean.
        let bytes = untracked_entries(STATUS_MAX_ENTRIES + 1);
        let status = parse_status_v2_z(&bytes).unwrap();
        assert!(
            status.overflow.is_some(),
            "expected overflow to be set: {status:?}"
        );
        let msg = status.overflow.as_ref().unwrap();
        assert!(
            msg.contains(&STATUS_MAX_ENTRIES.to_string()),
            "overflow message should mention the cap: {msg}"
        );
        // fail-closed: an overflowed status must never read as clean, and
        // must not have silently enumerated every path either (that would
        // defeat the point of the cap).
        assert!(!status.is_clean());
        assert!(status.untracked.len() <= STATUS_MAX_ENTRIES);
    }

    #[test]
    fn status_overflow_is_bounded_not_just_the_final_check() {
        // A much larger stream (10x the cap) still bails promptly with a
        // partial enumeration rather than collecting every path first.
        let bytes = untracked_entries(STATUS_MAX_ENTRIES * 10);
        let status = parse_status_v2_z(&bytes).unwrap();
        assert!(status.overflow.is_some());
        assert!(status.untracked.len() <= STATUS_MAX_ENTRIES);
        assert!(!status.is_clean());
    }
}
