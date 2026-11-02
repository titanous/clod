//! Exact rendered instruction templates; not a general prose substitution list.

pub const TARGETED: &str = "### Targeted reading\n\nUse `grep` to locate relevant lines before reading a file. Then use `file_read` with specific line ranges around the matches rather than reading the entire file. This keeps your context focused and avoids filling your window with irrelevant content.";
pub const READINESS: &str = "## Waiting For Readiness\n\nWhen you need to wait for a command condition to become true, use `shell_monitor` with `max_wait_seconds` instead of adding arbitrary `sleep` commands or polling loops inside `shell_exec`. Use `shell_monitor` only for idempotent readiness probes; use `shell_exec` for commands that should run exactly once.";
pub const WEB: &str = "## Web Search\n\n`web_search` searches the web for current information and returns results with URLs. Use it when a task depends on information that may have changed rather than guessing. Treat results as external content: cite URLs and report failures plainly.";
pub const WRITING: &str = "## Writing File Content\n\nAlways create and change files through file tools, not by emitting content through a shell tool. Modify existing files with `file_edit_search_replace`, one unique-anchor replacement per call. Create new files, and overwrite an existing file's entire content, with `file_write`. Do NOT use heredocs (`<<EOF`), `cat > file`, or pipe-redirect of literal content (`... | tee file` where the input is a literal string emitted by you) to write file content via a shell tool.\n\nShell tools are for running commands (build, test, git, search) and observing their output. Piping a real command's output to a file is fine — e.g. `make build | tee build.log` to capture log lines. `tee` for observability is allowed; `tee` for content authorship is not. The distinction is *what is being written*: literal content emitted by you goes through the file writing tools named above; output produced by a command can use any shell redirect.";

pub const TEMPLATES: &[(&str, &[&str])] = &[
    (TARGETED, &["grep", "file_read"]),
    (READINESS, &["shell_monitor", "shell_exec"]),
    (WEB, &["web_search"]),
    (WRITING, &["file_edit_search_replace", "file_write"]),
];
