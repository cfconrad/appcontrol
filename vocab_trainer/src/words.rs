/// One entry from the vocabulary file.
#[derive(Clone, Debug)]
pub struct VocabEntry {
    /// The word to display (question side).
    pub word: String,
    /// The correct translation.
    pub correct: String,
    /// Explicit wrong answer options supplied in the file.
    pub wrong: Vec<String>,
}

/// Parse a vocabulary text file.
///
/// Format (one entry per line):
/// ```text
/// Apfel = Apple Banana Orange Grape
/// "guten Morgen" = "good morning" "good evening" "good night" "good afternoon"
/// 'Wie geht es dir' = 'How are you' 'What is that' 'Where are you'
/// ```
///
/// * Tokens separated by whitespace; tokens **with spaces** must be wrapped in `'` or `"`.
/// * Format: `<word> = <correct_answer> [<wrong1> …]`
/// * Empty lines and lines starting with `#` are ignored.
pub fn parse_words_file(path: &str) -> Result<Vec<VocabEntry>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read words file {path:?}: {e}"))?;

    let mut entries = Vec::new();

    for (line_no, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let eq_pos = match line.find('=') {
            Some(p) => p,
            None => {
                eprintln!(
                    "vocab: line {} missing '=', skipping: {line:?}",
                    line_no + 1
                );
                continue;
            }
        };

        let word_part = line[..eq_pos].trim();
        let rest = line[eq_pos + 1..].trim();

        let word_tokens = tokenize(word_part);
        if word_tokens.len() != 1 {
            eprintln!(
                "vocab: line {} left-hand side must be a single token, skipping: {line:?}",
                line_no + 1
            );
            continue;
        }
        let word = word_tokens.into_iter().next().unwrap();

        let rhs = tokenize(rest);
        if rhs.is_empty() {
            eprintln!("vocab: line {} has no answers, skipping", line_no + 1);
            continue;
        }

        let correct = rhs[0].clone();
        let wrong = rhs[1..].to_vec();

        entries.push(VocabEntry {
            word,
            correct,
            wrong,
        });
    }

    if entries.is_empty() {
        return Err(format!("no vocabulary entries found in {path:?}"));
    }

    Ok(entries)
}

/// Tokenize a string respecting single- and double-quoted tokens.
pub fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();

    loop {
        // Skip whitespace.
        while matches!(chars.peek(), Some(' ' | '\t')) {
            chars.next();
        }

        match chars.peek() {
            None => break,
            Some(&'"') | Some(&'\'') => {
                let quote = chars.next().unwrap();
                let mut tok = String::new();
                loop {
                    match chars.next() {
                        None => break,
                        Some(c) if c == quote => break,
                        Some(c) => tok.push(c),
                    }
                }
                tokens.push(tok);
            }
            Some(_) => {
                let mut tok = String::new();
                loop {
                    match chars.peek() {
                        None | Some(' ') | Some('\t') => break,
                        _ => tok.push(chars.next().unwrap()),
                    }
                }
                if !tok.is_empty() {
                    tokens.push(tok);
                }
            }
        }
    }

    tokens
}

/// Serialize a token, quoting with `"` if it contains whitespace.
pub fn quote_token(s: &str) -> String {
    if s.chars().any(|c| c == ' ' || c == '\t') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}
