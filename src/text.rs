//! Text normalization shared by the indexer, the ranker and the preview.
//!
//! Normalized text is lowercase alphanumeric tokens separated by single spaces.
//! Each message becomes one line, and a field is " tok tok \n tok tok \n ".
//! Every token is preceded by a space, so a whole-word match for `tok` is
//! " tok" followed by a space, and a phrase can never span two messages.

/// Tokens longer than this are hashes, ids or encoded data, not words.
const MAX_TOKEN_CHARS: usize = 40;

/// A whitespace-delimited word that is long and has few separators is
/// almost always base64, a hash or minified data. Splitting it on
/// punctuation would produce random short tokens like "p0", so drop it whole.
/// Long URLs and paths survive because they are full of separators.
pub fn is_junk_word(word: &str) -> bool {
    let n = word.chars().count();
    if n <= MAX_TOKEN_CHARS {
        return false;
    }
    if n > 300 {
        return true;
    }
    let seps = word
        .chars()
        .filter(|c| matches!(c, '-' | '_' | '.' | '/' | ':' | '?' | '=' | '&' | '#' | ','))
        .count();
    seps * 12 < n
}

/// Append the normalized tokens of `raw` to `out` as "tok tok ".
/// Returns the number of tokens written. `out` must already end with a space.
pub fn push_tokens(raw: &str, out: &mut String) -> u32 {
    let mut count = 0;
    let mut tok = String::new();
    let mut tok_chars = 0;
    let flush = |tok: &mut String, tok_chars: &mut usize, out: &mut String, count: &mut u32| {
        if *tok_chars > 0 && *tok_chars <= MAX_TOKEN_CHARS {
            out.push_str(tok);
            out.push(' ');
            *count += 1;
        }
        tok.clear();
        *tok_chars = 0;
    };
    for word in raw.split_whitespace() {
        if is_junk_word(word) {
            continue;
        }
        for ch in word.chars() {
            if ch.is_alphanumeric() {
                tok.extend(ch.to_lowercase());
                tok_chars += 1;
            } else {
                flush(&mut tok, &mut tok_chars, out, &mut count);
            }
        }
        flush(&mut tok, &mut tok_chars, out, &mut count);
    }
    count
}

/// Normalize a query or a single message into a token list.
pub fn tokens(raw: &str) -> Vec<String> {
    let mut s = String::from(" ");
    push_tokens(raw, &mut s);
    s.split(' ').filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// Builds a normalized field from messages, one line per message.
#[derive(Default)]
pub struct FieldBuilder {
    pub text: String,
    pub len: u32,
}

impl FieldBuilder {
    pub fn push(&mut self, raw: &str) {
        if self.text.is_empty() {
            self.text.push(' ');
        }
        let n = push_tokens(raw, &mut self.text);
        if n > 0 {
            self.text.push_str("\n ");
            self.len += n;
        }
    }
}

/// Precompiled search for a token sequence inside normalized text.
pub struct Pattern {
    finder: memchr::memmem::Finder<'static>,
    len: usize,
}

/// Occurrence counts of a pattern in one normalized field.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Hits {
    /// The pattern ended on a word boundary.
    pub whole: u32,
    /// The last token matched only as a prefix of a longer word.
    pub prefix: u32,
}

impl Pattern {
    pub fn new(tokens: &[String]) -> Pattern {
        let needle = format!(" {}", tokens.join(" "));
        let len = needle.len();
        Pattern { finder: memchr::memmem::Finder::new(needle.as_bytes()).into_owned(), len }
    }

    pub fn count(&self, hay: &str) -> Hits {
        let bytes = hay.as_bytes();
        let mut hits = Hits::default();
        for pos in self.finder.find_iter(bytes) {
            match bytes.get(pos + self.len) {
                Some(b' ') => hits.whole += 1,
                _ => hits.prefix += 1,
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_punctuation_and_lowercases() {
        assert_eq!(tokens("mcp__claude-in_chrome MCP-Toolkit p0!"), ["mcp", "claude", "in", "chrome", "mcp", "toolkit", "p0"]);
    }

    #[test]
    fn drops_base64_but_keeps_urls() {
        let b64 = "SnwYKEAgSGAI4AUIIdGhpbmtpbmcSDJQeXv89DcC9af1QlBoMv6/fQ/TAcKN/wBAIIjBwd1bt7+seKMfQUiJG1rWAQmfz4";
        assert!(is_junk_word(b64));
        assert!(tokens(b64).is_empty());
        let url = "https://linear.app/standard-template-labs/issue/TKT-1210/change-specific-user-permissions";
        assert!(!is_junk_word(url));
        assert!(tokens(url).contains(&"tkt".to_string()));
    }

    #[test]
    fn counts_whole_words_prefixes_and_adjacent_repeats() {
        let mut f = FieldBuilder::default();
        f.push("mcp mcp mcpServers");
        f.push("the MCP gateway");
        let p = Pattern::new(&["mcp".into()]);
        assert_eq!(p.count(&f.text), Hits { whole: 3, prefix: 1 });
    }

    #[test]
    fn phrases_do_not_cross_messages() {
        let mut f = FieldBuilder::default();
        f.push("a connection");
        f.push("timeout b");
        let p = Pattern::new(&["connection".into(), "timeout".into()]);
        assert_eq!(p.count(&f.text), Hits::default());
        f.push("got a connection timeout");
        assert_eq!(p.count(&f.text).whole, 1);
    }
}
