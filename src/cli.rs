use crate::run;
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "thence", version)]
#[command(
    about = "Spec-driven supervisor for long-horizon coding runs",
    long_about = "thence translates a markdown spec into an internal plan, executes implementer/reviewer/checks loops, and records resumable run state."
)]
#[command(arg_required_else_help = true)]
#[command(after_long_help = "Examples:
  thence run spec.md --agent codex --checks \"cargo check;cargo test\"
  thence questions --run <RUN_ID>
  thence answer --run <RUN_ID> --question <QUESTION_ID> --text \"...\"
  thence resume --run <RUN_ID>
  thence completion zsh > ~/.zsh/completions/_thence
  thence man > thence.1

Docs: https://github.com/David-Factor/thence#readme
Issues: https://github.com/David-Factor/thence/issues")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(
        about = "Start a new supervised run from a markdown spec",
        long_about = "Start a supervised run from a markdown spec. thence translates the spec to an internal plan, executes implementer/reviewer attempts, runs deterministic checks, and records resumable state."
    )]
    #[command(arg_required_else_help = true)]
    #[command(after_long_help = "Examples:
  thence run spec.md
  thence run spec.md --agent codex --checks \"cargo check;cargo test\"
  thence run spec.md --simulate")]
    Run {
        #[arg(value_name = "PLAN_FILE", help = "Path to markdown spec file")]
        plan_file: PathBuf,
        #[arg(
            long,
            default_value = "codex",
            value_name = "PROVIDER",
            help = "Agent provider to use (only codex is supported in this version)"
        )]
        agent: String,
        #[arg(
            long,
            default_value_t = 2,
            value_name = "N",
            help = "Implementer worker count"
        )]
        workers: usize,
        #[arg(
            long,
            default_value_t = 1,
            value_name = "N",
            help = "Reviewer worker count"
        )]
        reviewers: usize,
        #[arg(
            long,
            value_name = "CMDS",
            help = "Semicolon-separated checks commands (e.g. \"cargo check;cargo test\")"
        )]
        checks: Option<String>,
        #[arg(long, help = "Run with stubbed/simulated agent behavior")]
        simulate: bool,
        #[arg(long, value_name = "PATH", help = "Write NDJSON event log to file")]
        log: Option<PathBuf>,
        #[arg(
            long,
            help = "Resume flow via run command (prefer `thence resume --run <RUN_ID>`)"
        )]
        resume: bool,
        #[arg(
            long,
            value_name = "RUN_ID",
            help = "Explicit run ID for new/resumed run"
        )]
        run_id: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to state DB (default: $XDG_STATE_HOME/thence/state.db)"
        )]
        state_db: Option<PathBuf>,
        #[arg(
            long,
            help = "Allow run completion when some tasks terminal-fail but others succeed"
        )]
        allow_partial_completion: bool,
        #[arg(long, help = "Trust per-task checks returned by plan translator")]
        trust_plan_checks: bool,
        #[arg(long, help = "Enable interactive mode for supporting agent adapters")]
        interactive: bool,
        #[arg(
            long,
            value_name = "SECS",
            help = "Hard timeout in seconds for implementer/reviewer attempts"
        )]
        attempt_timeout_secs: Option<u64>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Write translated SPL plan to this file for debugging"
        )]
        debug_dump_spl: Option<PathBuf>,
    },
    #[command(about = "List unresolved questions for a run")]
    #[command(arg_required_else_help = true)]
    #[command(after_long_help = "Example:
  thence questions --run <RUN_ID>")]
    Questions {
        #[arg(long, value_name = "RUN_ID", help = "Run ID to inspect")]
        run: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to state DB (default: $XDG_STATE_HOME/thence/state.db)"
        )]
        state_db: Option<PathBuf>,
    },
    #[command(about = "Answer a question opened during a run")]
    #[command(arg_required_else_help = true)]
    #[command(after_long_help = "Example:
  thence answer --run <RUN_ID> --question <QUESTION_ID> --text \"approve\"")]
    Answer {
        #[arg(long, value_name = "RUN_ID", help = "Run ID that owns the question")]
        run: String,
        #[arg(long, value_name = "QUESTION_ID", help = "Question ID to answer")]
        question: String,
        #[arg(long, value_name = "TEXT", help = "Answer text")]
        text: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to state DB (default: $XDG_STATE_HOME/thence/state.db)"
        )]
        state_db: Option<PathBuf>,
    },
    #[command(about = "Resume a paused or interrupted run")]
    #[command(arg_required_else_help = true)]
    #[command(after_long_help = "Example:
  thence resume --run <RUN_ID>")]
    Resume {
        #[arg(long, value_name = "RUN_ID", help = "Run ID to resume")]
        run: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to state DB (default: $XDG_STATE_HOME/thence/state.db)"
        )]
        state_db: Option<PathBuf>,
    },
    #[command(about = "Inspect current state for a run")]
    #[command(arg_required_else_help = true)]
    #[command(after_long_help = "Example:
  thence inspect --run <RUN_ID>")]
    Inspect {
        #[arg(long, value_name = "RUN_ID", help = "Run ID to inspect")]
        run: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to state DB (default: $XDG_STATE_HOME/thence/state.db)"
        )]
        state_db: Option<PathBuf>,
    },
    #[command(
        about = "Generate shell completion script",
        long_about = "Generate shell completion script for your shell. Redirect output to your shell completion directory."
    )]
    #[command(arg_required_else_help = true)]
    #[command(after_long_help = "Examples:
  thence completion bash > ~/.local/share/bash-completion/completions/thence
  thence completion zsh > ~/.zsh/completions/_thence
  thence completion fish > ~/.config/fish/completions/thence.fish")]
    Completion {
        #[arg(value_enum, value_name = "SHELL", help = "Target shell")]
        shell: Shell,
    },
    #[command(
        about = "Generate a man page",
        long_about = "Generate a roff man page for thence."
    )]
    #[command(after_long_help = "Examples:
  thence man > thence.1
  thence man --output docs/thence.1")]
    Man {
        #[arg(
            long,
            value_name = "PATH",
            help = "Write man page to file (stdout when omitted)"
        )]
        output: Option<PathBuf>,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            plan_file,
            agent,
            workers,
            reviewers,
            checks,
            simulate,
            log,
            resume,
            run_id,
            state_db,
            allow_partial_completion,
            trust_plan_checks,
            interactive,
            attempt_timeout_secs,
            debug_dump_spl,
        } => {
            let cfg = run::RunCommand {
                plan_file,
                agent,
                workers,
                reviewers,
                checks,
                simulate,
                log,
                resume,
                run_id,
                state_db,
                allow_partial_completion,
                trust_plan_checks,
                interactive,
                attempt_timeout_secs,
                debug_dump_spl,
            };
            run::execute_run(cfg)
        }
        Commands::Questions {
            run: run_id,
            state_db,
        } => run::list_questions(&run_id, state_db),
        Commands::Answer {
            run: run_id,
            question,
            text,
            state_db,
        } => run::answer_question(&run_id, &question, &text, state_db),
        Commands::Resume {
            run: run_id,
            state_db,
        } => run::resume_run(&run_id, state_db),
        Commands::Inspect {
            run: run_id,
            state_db,
        } => run::inspect_run(&run_id, state_db),
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut io::stdout());
            Ok(())
        }
        Commands::Man { output } => {
            let man = clap_mangen::Man::new(Cli::command());
            match output {
                Some(path) => {
                    let mut bytes = Vec::new();
                    man.render(&mut bytes)?;
                    fs::write(path, bytes)?;
                }
                None => {
                    man.render(&mut io::stdout())?;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_help_contract_has_simulate_and_no_legacy_flags() {
        let cmd = Cli::command();
        let run = cmd
            .get_subcommands()
            .find(|sub| sub.get_name() == "run")
            .expect("run subcommand");
        let arg_ids = run
            .get_arguments()
            .map(|arg| arg.get_id().as_str().to_string())
            .collect::<Vec<_>>();

        assert!(arg_ids.contains(&"simulate".to_string()));
        assert!(!arg_ids.contains(&"reconfigure_checks".to_string()));
        assert!(!arg_ids.contains(&"no_checks_file".to_string()));
        assert!(!arg_ids.contains(&"agent_cmd".to_string()));
        assert!(!arg_ids.contains(&"agent_cmd_codex".to_string()));
        assert!(!arg_ids.contains(&"agent_cmd_claude".to_string()));
        assert!(!arg_ids.contains(&"agent_cmd_opencode".to_string()));
    }

    #[test]
    fn removed_run_flags_are_rejected_by_cli() {
        assert!(Cli::try_parse_from(["thence", "run", "spec.md", "--reconfigure-checks"]).is_err());
        assert!(Cli::try_parse_from(["thence", "run", "spec.md", "--agent-cmd", "codex"]).is_err());
    }
}
