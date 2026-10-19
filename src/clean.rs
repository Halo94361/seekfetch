//! Post-processing for DeepSeek extractor output.
//!
//! The extractor escapes every ASCII punctuation mark with a backslash
//! (the CommonMark-escapable set), which is pure noise for model consumption.
//! Prose is unescaped; fenced code blocks are kept verbatim because a
//! legitimate backslash there is indistinguishable from an artifact — instead
//! a hint line is injected at the top of any block that contains artifacts.

const CODE_BLOCK_HINT: &str = "[seekfetch note] this code block may contain extractor escape artifacts (e.g. \\( or \\{ standing for the literal character); interpret them by meaning";

pub fn clean_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 128);
    let mut lines = text.lines();

    while let Some(line) = lines.next() {
        match opening_fence(line) {
            None => {
                out.push_str(&unescape_prose(line));
                out.push('\n');
            }
            Some((fence_char, fence_len)) => {
                let mut block: Vec<&str> = Vec::new();
                let mut closing: Option<&str> = None;
                for next in lines.by_ref() {
                    if is_closing_fence(next, fence_char, fence_len) {
                        closing = Some(next);
                        break;
                    }
                    block.push(next);
                }

                out.push_str(line);
                out.push('\n');
                if block.iter().any(|l| has_artifact(l)) {
                    out.push_str(CODE_BLOCK_HINT);
                    out.push('\n');
                }
                for block_line in block {
                    out.push_str(block_line);
                    out.push('\n');
                }
                if let Some(closing) = closing {
                    out.push_str(closing);
                    out.push('\n');
                }
            }
        }
    }

    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

fn unescape_prose(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        // char::is_ascii_punctuation is exactly the CommonMark-escapable set.
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            out.push(chars.next().unwrap());
        } else {
            out.push(c);
        }
    }
    out
}

fn has_artifact(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            return true;
        }
    }
    false
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let fence_char = chars.next()?;
    if fence_char != '`' && fence_char != '~' {
        return None;
    }
    let mut len = 1;
    for c in chars {
        if c == fence_char {
            len += 1;
        } else {
            break;
        }
    }
    (len >= 3).then_some((fence_char, len))
}

fn is_closing_fence(line: &str, fence_char: char, fence_len: usize) -> bool {
    let trimmed = line.trim_start();
    let len = trimmed.chars().take_while(|&c| c == fence_char).count();
    len >= fence_len && trimmed[len..].trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_is_unescaped() {
        let input = "Welcome\\! Use components\\. It\\'s fine\\, really\\.";
        assert_eq!(
            clean_markdown(input),
            "Welcome! Use components. It's fine, really."
        );
    }

    #[test]
    fn backslash_before_non_punctuation_is_kept() {
        assert_eq!(
            clean_markdown(r"regex \d+ and C:\temp"),
            r"regex \d+ and C:\temp"
        );
    }

    #[test]
    fn code_block_is_verbatim_and_gets_hint() {
        let input = "intro\\. text\n```rust\nfn main\\(\\) \\{}\n```\nafter\\!\n";
        let expected = "intro. text\n```rust\n".to_string()
            + CODE_BLOCK_HINT
            + "\nfn main\\(\\) \\{}\n```\nafter!\n";
        assert_eq!(clean_markdown(input), expected);
    }

    #[test]
    fn clean_code_block_gets_no_hint() {
        let input = "```sh\nls -la /tmp\n```";
        assert_eq!(clean_markdown(input), input);
    }

    #[test]
    fn tilde_fence_and_unclosed_fence() {
        let input = "~~~\npath C:\\\\tmp\n~~~";
        let expected = "~~~\n".to_string() + CODE_BLOCK_HINT + "\npath C:\\\\tmp\n~~~";
        assert_eq!(clean_markdown(input), expected);

        let unclosed = "before\\.\n```\ncode \\(x\\)";
        let expected = "before.\n```\n".to_string() + CODE_BLOCK_HINT + "\ncode \\(x\\)";
        assert_eq!(clean_markdown(unclosed), expected);
    }

    #[test]
    fn escaped_backslash_collapses() {
        assert_eq!(clean_markdown(r"a\\b"), r"a\b");
    }
}
