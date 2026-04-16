use std::collections::HashMap;
use std::time::Instant;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusqlite::Connection;

use crate::words::VocabEntry;
use crate::{progress, words};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub struct QuizConfig {
    pub correct_needed: u32,
    pub extra_minutes: u32,
    pub words_file: String,
    pub progress_db: String,
    pub username: String,
}

// ---------------------------------------------------------------------------
// Weighted pool
// ---------------------------------------------------------------------------

/// Map a correct-answer count to a repetition weight.
/// Words answered fewer times appear more often.
fn tier_weight(correct: u32) -> usize {
    match correct {
        0 => 8,
        1 => 4,
        2 => 2,
        _ => 1,
    }
}

/// Build a flat index pool where each entry index appears `tier_weight` times.
/// Picking a random element from this pool gives the desired distribution.
pub fn build_weighted_pool(entries: &[VocabEntry], progress: &HashMap<String, u32>) -> Vec<usize> {
    let mut pool = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let count = progress.get(&entry.word).copied().unwrap_or(0);
        let w = tier_weight(count);
        for _ in 0..w {
            pool.push(i);
        }
    }
    pool
}

// ---------------------------------------------------------------------------
// Quiz state
// ---------------------------------------------------------------------------

pub struct QuizState {
    pub config: QuizConfig,
    pub entries: Vec<VocabEntry>,
    pub pool: Vec<usize>,
    pub progress: HashMap<String, u32>,
    pub progress_conn: Connection,
    /// Index into `entries` for the current question.
    pub current: usize,
    /// Four answer options (exactly).
    pub choices: Vec<String>,
    /// Which index in `choices` is the correct answer.
    pub correct_idx: usize,
    /// How many correct answers so far this session.
    pub score: u32,
    /// Flash state: (was_correct, chosen_button_idx, timestamp).
    pub flash: Option<(bool, usize, Instant)>,
    rng: StdRng,
}

impl QuizState {
    pub fn new(config: QuizConfig) -> Result<Self, String> {
        let entries = words::parse_words_file(&config.words_file)?;
        if entries.len() < 2 {
            return Err("words file needs at least 2 entries for a quiz".into());
        }

        let progress_conn = progress::open_progress_db(&config.progress_db)
            .map_err(|e| format!("cannot open progress DB {:?}: {e}", config.progress_db))?;
        let prog = progress::load_progress(&progress_conn, &config.username);
        let pool = build_weighted_pool(&entries, &prog);

        let mut rng = StdRng::from_entropy();
        let current = pool[rng.gen_range(0..pool.len())];
        let choices = make_choices(&entries, current, &mut rng);
        let correct_idx = choices
            .iter()
            .position(|c| c == &entries[current].correct)
            .unwrap_or(0);

        Ok(QuizState {
            config,
            entries,
            pool,
            progress: prog,
            progress_conn,
            current,
            choices,
            correct_idx,
            score: 0,
            flash: None,
            rng,
        })
    }

    /// Move to a new random question (weighted).
    pub fn next_question(&mut self) {
        let idx = self.pool[self.rng.gen_range(0..self.pool.len())];
        self.current = idx;
        self.choices = make_choices(&self.entries, idx, &mut self.rng);
        self.correct_idx = self.choices
            .iter()
            .position(|c| c == &self.entries[idx].correct)
            .unwrap_or(0);
    }

    /// Record an answer. Returns `true` if correct.
    /// On correct answer: increments DB count, rebuilds pool.
    pub fn answer(&mut self, chosen_idx: usize) -> bool {
        let correct = chosen_idx == self.correct_idx;
        if correct {
            let word = self.entries[self.current].word.clone();
            let _ = progress::increment_correct(
                &self.progress_conn,
                &self.config.username,
                &word,
            );
            let count = self.progress.entry(word).or_insert(0);
            *count += 1;
            self.score += 1;
            // Rebuild so the newly learned word drops in frequency immediately.
            self.pool = build_weighted_pool(&self.entries, &self.progress);
        }
        self.flash = Some((correct, chosen_idx, Instant::now()));
        correct
    }
}

// ---------------------------------------------------------------------------
// Choice generation
// ---------------------------------------------------------------------------

/// Build a shuffled 4-element answer list for `entries[current]`.
///
/// Wrong answers come first from the entry's own explicit wrongs,
/// then from other entries' correct answers (randomly sampled).
fn make_choices(entries: &[VocabEntry], current: usize, rng: &mut StdRng) -> Vec<String> {
    let entry = &entries[current];
    let correct = entry.correct.clone();

    // Collect wrong candidates: explicit wrongs first, then other entries' answers.
    let mut wrong_pool: Vec<String> = Vec::new();
    for w in &entry.wrong {
        if w != &correct && !wrong_pool.contains(w) {
            wrong_pool.push(w.clone());
        }
    }
    for (i, e) in entries.iter().enumerate() {
        if i != current && e.correct != correct && !wrong_pool.contains(&e.correct) {
            wrong_pool.push(e.correct.clone());
        }
    }

    // Shuffle the portion that comes from other entries (beyond entry.wrong).
    let fixed = entry.wrong.len().min(wrong_pool.len());
    let tail = &mut wrong_pool[fixed..];
    let tail_len = tail.len();
    for i in (1..tail_len).rev() {
        let j = rng.gen_range(0..=i);
        tail.swap(i, j);
    }

    // Build choices: 1 correct + up to 3 wrong.
    let mut choices = vec![correct];
    choices.extend(wrong_pool.into_iter().take(3));
    while choices.len() < 4 {
        choices.push("—".to_string());
    }

    // Shuffle all four.
    let len = choices.len();
    for i in (1..len).rev() {
        let j = rng.gen_range(0..=i);
        choices.swap(i, j);
    }

    choices
}
