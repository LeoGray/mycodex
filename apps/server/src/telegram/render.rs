use crate::state::{ApprovalRule, RepoRecord, ThreadRecord, ThreadSurface};

pub const TELEGRAM_MESSAGE_LIMIT: usize = 3800;

#[derive(Debug, Clone, Default)]
pub struct ProgressView {
    pub repo_name: String,
    pub thread_title: String,
    pub status: String,
    pub assistant_text: String,
    pub command_output_tail: String,
    pub diff_preview: String,
}

pub fn render_help() -> String {
    [
        "MyCodex commands",
        "",
        "/start",
        "/help",
        "/status",
        "/abort",
        "/approval",
        "/approval list",
        "/approval remove <rule>",
        "/approval clear",
        "/repo",
        "/repo list",
        "/repo use <name>",
        "/repo clone <git_url> [dir_name]",
        "/repo status",
        "/repo rescan",
        "/thread",
        "/thread list",
        "/thread new",
        "/thread use <thread>",
        "/thread status",
        "",
        "Send /repo, /thread, or /approval to open a submenu.",
        "Send plain text to talk to the active repo/thread.",
    ]
    .join("\n")
}

pub fn render_status(
    active_repo: Option<&RepoRecord>,
    active_thread: Option<&ThreadRecord>,
    runtime_repo_id: Option<&str>,
    active_turn_id: Option<&str>,
    pending_summary: Option<&str>,
    approval_rule_count: usize,
) -> String {
    let repo_line = active_repo
        .map(|repo| format!("repo: {} ({})", repo.name, repo.path.display()))
        .unwrap_or_else(|| "repo: none".to_string());
    let thread_line = active_thread
        .map(|thread| {
            format!(
                "thread: {} [{}]",
                thread.title,
                short_id(&thread.local_thread_id)
            )
        })
        .unwrap_or_else(|| "thread: none".to_string());
    let runtime_line = match (active_repo, runtime_repo_id) {
        (Some(repo), Some(active_runtime)) if repo.repo_id == active_runtime => {
            "runtime: running".to_string()
        }
        (Some(_), Some(_)) => "runtime: running on another repo".to_string(),
        _ => "runtime: stopped".to_string(),
    };
    let turn_line = active_turn_id
        .map(|turn| format!("turn: {}", short_id(turn)))
        .unwrap_or_else(|| "turn: none".to_string());
    let pending_line = pending_summary
        .map(|value| format!("pending: {value}"))
        .unwrap_or_else(|| "pending: none".to_string());
    let approval_line = format!("approval rules: {approval_rule_count}");

    [
        repo_line,
        thread_line,
        runtime_line,
        turn_line,
        pending_line,
        approval_line,
    ]
    .join("\n")
}

pub fn render_repo_list(repos: &[RepoRecord], active_repo_id: Option<&str>) -> String {
    if repos.is_empty() {
        return "No repos registered. Use /repo clone or /repo rescan.".to_string();
    }

    let mut lines = vec!["Repos".to_string(), String::new()];
    for repo in repos {
        let marker = if Some(repo.repo_id.as_str()) == active_repo_id {
            "*"
        } else {
            "-"
        };
        lines.push(format!(
            "{} {} [{}] threads={}",
            marker,
            repo.name,
            short_id(&repo.repo_id),
            repo.threads_for_surface(ThreadSurface::Telegram).len()
        ));
    }
    lines.join("\n")
}

pub fn render_repo_status(repo: &RepoRecord) -> String {
    let active = repo.active_thread_local_id.as_deref().unwrap_or("none");
    [
        format!("repo: {}", repo.name),
        format!("path: {}", repo.path.display()),
        format!(
            "origin: {}",
            repo.origin_url.as_deref().unwrap_or("unknown")
        ),
        format!(
            "threads: {}",
            repo.threads_for_surface(ThreadSurface::Telegram).len()
        ),
        format!("active thread: {}", short_id(active)),
    ]
    .join("\n")
}

pub fn render_approval_rules(repo: &RepoRecord, rules: &[&ApprovalRule]) -> String {
    if rules.is_empty() {
        return format!("Repo {} has no approval rules.", repo.name);
    }

    let mut lines = vec![format!("Approval rules for {}", repo.name), String::new()];
    for (index, rule) in rules.iter().enumerate() {
        lines.push(format!(
            "{}. {} [{}]",
            index + 1,
            trim_middle(&rule.command, 120),
            short_id(&rule.rule_id)
        ));
    }
    lines.join("\n")
}

pub fn render_thread_list(repo: &RepoRecord) -> String {
    let threads = repo.threads_for_surface(ThreadSurface::Telegram);
    if threads.is_empty() {
        return format!("Repo {} has no threads yet.", repo.name);
    }

    let mut lines = vec![format!("Threads for {}", repo.name), String::new()];
    for (index, thread) in threads.into_iter().enumerate() {
        let marker =
            if repo.active_thread_local_id.as_deref() == Some(thread.local_thread_id.as_str()) {
                "*"
            } else {
                "-"
            };
        lines.push(format!(
            "{} {}. {} [{}]",
            marker,
            index + 1,
            thread.title,
            short_id(&thread.local_thread_id)
        ));
    }
    lines.join("\n")
}

pub fn render_thread_status(repo: &RepoRecord, thread: Option<&ThreadRecord>) -> String {
    match thread {
        Some(thread) => [
            format!("repo: {}", repo.name),
            format!("thread: {}", thread.title),
            format!("local id: {}", thread.local_thread_id),
            format!("codex id: {}", thread.codex_thread_id),
            format!("status: {:?}", thread.status),
        ]
        .join("\n"),
        None => format!("Repo {} has no active thread.", repo.name),
    }
}

pub fn render_progress(progress: &ProgressView) -> String {
    let mut lines = vec![
        format!("repo: {}", progress.repo_name),
        format!("thread: {}", progress.thread_title),
        format!("status: {}", progress.status),
    ];

    if !progress.assistant_text.is_empty() {
        lines.push(String::new());
        lines.push("assistant".to_string());
        lines.push(trim_middle(&progress.assistant_text, 2200));
    }

    if !progress.command_output_tail.is_empty() {
        lines.push(String::new());
        lines.push("command output".to_string());
        lines.push(trim_start(&progress.command_output_tail, 1200));
    }

    if !progress.diff_preview.is_empty() {
        lines.push(String::new());
        lines.push("diff".to_string());
        lines.push(trim_middle(&progress.diff_preview, 800));
    }

    trim_middle(&lines.join("\n"), TELEGRAM_MESSAGE_LIMIT)
}

pub fn render_command_approval(
    repo_name: &str,
    thread_title: &str,
    cwd: Option<&str>,
    command: Option<&str>,
    reason: Option<&str>,
) -> String {
    let mut lines = vec![
        "Command approval needed".to_string(),
        format!("repo: {}", repo_name),
        format!("thread: {}", thread_title),
    ];
    if let Some(cwd) = cwd {
        lines.push(format!("cwd: {cwd}"));
    }
    if let Some(command) = command {
        lines.push(format!("command: {command}"));
    }
    if let Some(reason) = reason {
        lines.push(format!("reason: {reason}"));
    }
    lines.join("\n")
}

pub fn render_file_approval(
    repo_name: &str,
    thread_title: &str,
    paths: &[String],
    reason: Option<&str>,
    diff_preview: &str,
) -> String {
    let mut lines = vec![
        "File approval needed".to_string(),
        format!("repo: {}", repo_name),
        format!("thread: {}", thread_title),
        format!("paths: {}", paths.join(", ")),
    ];
    if let Some(reason) = reason {
        lines.push(format!("reason: {reason}"));
    }
    if !diff_preview.is_empty() {
        lines.push(String::new());
        lines.push(trim_middle(diff_preview, 1600));
    }
    trim_middle(&lines.join("\n"), TELEGRAM_MESSAGE_LIMIT)
}

pub fn render_repo_menu() -> String {
    "Choose list | use | clone | status | rescan for /repo.".to_string()
}

pub fn render_repo_use_menu(repos: &[RepoRecord]) -> String {
    if repos.is_empty() {
        return "No repos registered. Use /repo clone or /repo rescan.".to_string();
    }
    "Choose a repo for /repo use.".to_string()
}

pub fn render_repo_clone_menu() -> String {
    "Send /repo clone <git_url> [dir_name].".to_string()
}

pub fn render_thread_menu() -> String {
    "Choose list | new | use | status for /thread.".to_string()
}

pub fn render_thread_use_menu(repo: &RepoRecord) -> String {
    if repo.threads.is_empty() {
        return format!("Repo {} has no threads yet.", repo.name);
    }
    format!("Choose a thread in {} for /thread use.", repo.name)
}

pub fn render_approval_menu() -> String {
    "Choose list | remove | clear for /approval.".to_string()
}

pub fn render_approval_remove_menu(repo: &RepoRecord, rules: &[&ApprovalRule]) -> String {
    if rules.is_empty() {
        return format!("Repo {} has no approval rules.", repo.name);
    }
    format!("Choose an approval rule to remove from {}.", repo.name)
}

pub fn split_message(text: &str) -> Vec<String> {
    if text.len() <= TELEGRAM_MESSAGE_LIMIT {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if rest.len() <= TELEGRAM_MESSAGE_LIMIT {
            chunks.push(rest.to_string());
            break;
        }
        let candidate = &rest[..TELEGRAM_MESSAGE_LIMIT];
        let split_at = candidate.rfind('\n').unwrap_or(TELEGRAM_MESSAGE_LIMIT);
        chunks.push(rest[..split_at].to_string());
        rest = rest[split_at..].trim_start_matches('\n').trim_start();
    }
    chunks
}

pub fn short_id(value: &str) -> String {
    if value.len() <= 8 {
        value.to_string()
    } else {
        value[..8].to_string()
    }
}

pub fn title_from_text(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    trim_middle(&normalized, 72)
}

fn trim_middle(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    let head = max_len / 2;
    let tail = max_len.saturating_sub(head + 5);
    format!("{} ... {}", &value[..head], &value[value.len() - tail..])
}

fn trim_start(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    format!("... {}", &value[value.len() - max_len..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_message() {
        let message = "x".repeat(TELEGRAM_MESSAGE_LIMIT + 10);
        let chunks = split_message(&message);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn title_is_trimmed() {
        let title = title_from_text(&"word ".repeat(40));
        assert!(title.len() <= 72);
    }
}
