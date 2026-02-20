use crate::run;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "whence")]
#[command(about = "Simple supervisor runner", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run {
        plan_file: PathBuf,
        #[arg(long, default_value = "codex")]
        agent: String,
        #[arg(long, default_value_t = 2)]
        workers: usize,
        #[arg(long, default_value_t = 1)]
        reviewers: usize,
        #[arg(long)]
        checks: Option<String>,
        #[arg(long)]
        reconfigure_checks: bool,
        #[arg(long)]
        no_checks_file: bool,
        #[arg(long)]
        log: Option<PathBuf>,
        #[arg(long)]
        resume: bool,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        state_db: Option<PathBuf>,
        #[arg(long)]
        allow_partial_completion: bool,
        #[arg(long)]
        trust_plan_checks: bool,
        #[arg(long)]
        interactive: bool,
        #[arg(long)]
        debug_dump_spl: Option<PathBuf>,
        #[arg(long)]
        agent_cmd: Option<String>,
        #[arg(long)]
        agent_cmd_codex: Option<String>,
        #[arg(long)]
        agent_cmd_claude: Option<String>,
        #[arg(long)]
        agent_cmd_opencode: Option<String>,
    },
    Questions {
        #[arg(long)]
        run: String,
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    Answer {
        #[arg(long)]
        run: String,
        #[arg(long)]
        question: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    Resume {
        #[arg(long)]
        run: String,
        #[arg(long)]
        state_db: Option<PathBuf>,
    },
    Inspect {
        #[arg(long)]
        run: String,
        #[arg(long)]
        state_db: Option<PathBuf>,
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
            reconfigure_checks,
            no_checks_file,
            log,
            resume,
            run_id,
            state_db,
            allow_partial_completion,
            trust_plan_checks,
            interactive,
            debug_dump_spl,
            agent_cmd,
            agent_cmd_codex,
            agent_cmd_claude,
            agent_cmd_opencode,
        } => {
            let cfg = run::RunCommand {
                plan_file,
                agent,
                workers,
                reviewers,
                checks,
                reconfigure_checks,
                no_checks_file,
                log,
                resume,
                run_id,
                state_db,
                allow_partial_completion,
                trust_plan_checks,
                interactive,
                debug_dump_spl,
                agent_cmd,
                agent_cmd_codex,
                agent_cmd_claude,
                agent_cmd_opencode,
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
    }
}
