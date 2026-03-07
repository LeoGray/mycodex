#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInput {
    Command(Command),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Start,
    Help,
    Status,
    Abort,
    Repo(RepoCommand),
    Thread(ThreadCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoCommand {
    List,
    Use {
        repo: String,
    },
    Clone {
        git_url: String,
        dir_name: Option<String>,
    },
    Status,
    Rescan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadCommand {
    List,
    New,
    Use { thread: String },
    Status,
}

pub fn parse_user_input(text: &str) -> UserInput {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return UserInput::Text(trimmed.to_string());
    }

    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or_default();
    let command = command
        .split('@')
        .next()
        .unwrap_or(command)
        .trim_start_matches('/');

    match command {
        "start" => UserInput::Command(Command::Start),
        "help" => UserInput::Command(Command::Help),
        "status" => UserInput::Command(Command::Status),
        "abort" => UserInput::Command(Command::Abort),
        "repo" => UserInput::Command(Command::Repo(parse_repo_command(parts.collect()))),
        "thread" => UserInput::Command(Command::Thread(parse_thread_command(parts.collect()))),
        _ => UserInput::Text(trimmed.to_string()),
    }
}

fn parse_repo_command(args: Vec<&str>) -> RepoCommand {
    match args.as_slice() {
        ["list"] => RepoCommand::List,
        ["use", repo] => RepoCommand::Use {
            repo: (*repo).to_string(),
        },
        ["clone", git_url] => RepoCommand::Clone {
            git_url: (*git_url).to_string(),
            dir_name: None,
        },
        ["clone", git_url, dir_name] => RepoCommand::Clone {
            git_url: (*git_url).to_string(),
            dir_name: Some((*dir_name).to_string()),
        },
        ["status"] => RepoCommand::Status,
        ["rescan"] => RepoCommand::Rescan,
        _ => RepoCommand::Status,
    }
}

fn parse_thread_command(args: Vec<&str>) -> ThreadCommand {
    match args.as_slice() {
        ["list"] => ThreadCommand::List,
        ["new"] => ThreadCommand::New,
        ["use", thread] => ThreadCommand::Use {
            thread: (*thread).to_string(),
        },
        ["status"] => ThreadCommand::Status,
        _ => ThreadCommand::Status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_clone() {
        let input = parse_user_input("/repo clone https://github.com/openai/codex.git codex");
        assert_eq!(
            input,
            UserInput::Command(Command::Repo(RepoCommand::Clone {
                git_url: "https://github.com/openai/codex.git".into(),
                dir_name: Some("codex".into()),
            }))
        );
    }

    #[test]
    fn falls_back_to_text_for_unknown_command() {
        let input = parse_user_input("/hello world");
        assert_eq!(input, UserInput::Text("/hello world".into()));
    }

    #[test]
    fn parses_help_command() {
        let input = parse_user_input("/help");
        assert_eq!(input, UserInput::Command(Command::Help));
    }
}
