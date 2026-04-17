pub mod progress;
pub mod quiz;
pub mod words;

mod ui;

pub use quiz::QuizConfig;

/// Launch the vocabulary quiz window.
///
/// This function **exits the process** when the quiz finishes:
/// * exit 0 — the user reached `correct_needed`; prints `EARNED_SECS:<n>` to stdout first.
/// * exit 1 — the user clicked Quit.
/// * exit 2 — display / configuration error.
pub fn run_quiz(config: QuizConfig) {
    ui::run_quiz_window(config);
}

// ---------------------------------------------------------------------------
// Helpers re-exported for use by appcontrold
// ---------------------------------------------------------------------------

/// Print the progress table for `username` to stdout.
pub fn print_progress(progress_db: &str, username: &str) {
    let conn = match progress::open_progress_db(progress_db) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("vocab: cannot open progress DB: {e}");
            std::process::exit(1);
        }
    };
    let rows = match progress::list_progress(&conn, username) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("vocab: cannot read progress: {e}");
            std::process::exit(1);
        }
    };

    if rows.is_empty() {
        println!("no progress recorded for user {username:?}");
        return;
    }

    println!("{:<40}  correct", "word");
    println!("{}", "-".repeat(50));
    for (word, count) in &rows {
        println!("{:<40}  {count}", words::quote_token(word));
    }
}

/// Open an editor to let the user adjust correct-answer counts.
///
/// `words_file` is optional; if provided, words with 0 count are also shown.
pub fn edit_progress(progress_db: &str, words_file: Option<&str>, username: &str) {
    let conn = match progress::open_progress_db(progress_db) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("vocab: cannot open progress DB: {e}");
            std::process::exit(1);
        }
    };

    // Build map: word → count, seeded from DB.
    let mut prog = progress::load_progress(&conn, username);

    // If a words file was given, merge in words that have no DB entry yet.
    if let Some(path) = words_file {
        match words::parse_words_file(path) {
            Ok(entries) => {
                for e in entries {
                    prog.entry(e.word).or_insert(0);
                }
            }
            Err(e) => eprintln!("vocab: warning: {e}"),
        }
    }

    // Sort: by count asc, then word asc.
    let mut rows: Vec<(String, u32)> = prog.into_iter().collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    let tmp_path = format!("/tmp/vocab_progress_{username}_edit.txt");

    let mut content = format!(
        "# vocab progress for user: {username}\n\
         # Format: word  correct-count\n\
         # Increase the count to make a word appear less often (higher tier).\n\
         # Decrease the count to see it more often (lower tier).\n\
         # Tiers:  0 → weight 8,  1 → weight 4,  2 → weight 2,  3+ → weight 1\n\
         # Lines starting with '#' are ignored.\n\n"
    );
    for (word, count) in &rows {
        content.push_str(&format!("{} {count}\n", words::quote_token(word)));
    }

    std::fs::write(&tmp_path, &content).unwrap_or_else(|e| panic!("cannot write temp file: {e}"));

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .unwrap_or_else(|e| panic!("cannot launch editor {editor:?}: {e}"));

    if !status.success() {
        eprintln!("vocab: editor exited with non-zero status, aborting");
        std::process::exit(1);
    }

    let edited =
        std::fs::read_to_string(&tmp_path).unwrap_or_else(|e| panic!("cannot read temp file: {e}"));

    let mut updated = 0usize;
    for (line_no, raw) in edited.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let tokens = words::tokenize(line);
        if tokens.len() < 2 {
            eprintln!(
                "vocab edit: skipping malformed line {} (expected: word count): {line:?}",
                line_no + 1
            );
            continue;
        }
        let word = &tokens[0];
        let count: u32 = match tokens[1].parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!(
                    "vocab edit: invalid count {:?} on line {}, skipping",
                    tokens[1],
                    line_no + 1
                );
                continue;
            }
        };
        if let Err(e) = progress::set_correct(&conn, username, word, count) {
            eprintln!("vocab edit: DB error for {word:?}: {e}");
        } else {
            updated += 1;
        }
    }

    println!("vocab: progress updated ({updated} word(s)) for user {username:?}");
    let _ = std::fs::remove_file(&tmp_path);
}
