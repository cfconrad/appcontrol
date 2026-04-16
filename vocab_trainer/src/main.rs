use clap::{Args, Parser, Subcommand};
use vocab_trainer::QuizConfig;

#[derive(Parser)]
#[command(name = "vocab_trainer", about = "Vocabulary trainer with spaced repetition")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the vocabulary quiz (graphical window).
    Quiz(QuizArgs),
    /// Show correct-answer counts for a user.
    List(ListArgs),
    /// Edit correct-answer counts for a user in $EDITOR.
    Edit(EditArgs),
}

// ── quiz ────────────────────────────────────────────────────────────────────

#[derive(Args)]
struct QuizArgs {
    /// Path to the vocabulary text file.
    #[arg(long)]
    words_file: String,

    /// Path to the SQLite progress database (created if absent).
    #[arg(long)]
    progress_db: String,

    /// Username whose progress is tracked.
    #[arg(long)]
    user: String,

    /// Number of correct answers needed to earn extra time.
    #[arg(long, default_value = "5")]
    correct: u32,

    /// Minutes of extra time awarded on success.
    #[arg(long, default_value = "15")]
    extra_minutes: u32,
}

// ── list ────────────────────────────────────────────────────────────────────

#[derive(Args)]
struct ListArgs {
    /// Path to the SQLite progress database.
    #[arg(long)]
    progress_db: String,

    /// Username to display progress for.
    #[arg(long)]
    user: String,
}

// ── edit ────────────────────────────────────────────────────────────────────

#[derive(Args)]
struct EditArgs {
    /// Path to the SQLite progress database.
    #[arg(long)]
    progress_db: String,

    /// Username whose progress to edit.
    #[arg(long)]
    user: String,

    /// Optional vocabulary file — used to pre-populate words with zero count.
    #[arg(long)]
    words_file: Option<String>,
}

// ── main ────────────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Quiz(a) => {
            vocab_trainer::run_quiz(QuizConfig {
                correct_needed: a.correct,
                extra_minutes: a.extra_minutes,
                words_file: a.words_file,
                progress_db: a.progress_db,
                username: a.user,
            });
        }
        Command::List(a) => {
            vocab_trainer::print_progress(&a.progress_db, &a.user);
        }
        Command::Edit(a) => {
            vocab_trainer::edit_progress(
                &a.progress_db,
                a.words_file.as_deref(),
                &a.user,
            );
        }
    }
}
