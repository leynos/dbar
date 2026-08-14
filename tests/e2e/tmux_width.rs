//! Display-width accounting for rendered tmux status lines.
//!
//! dbar emits tmux markup rather than ANSI escapes, so a rendered line cannot
//! be measured by character count. `#[...]` style tags are markup and occupy no
//! columns, and `##` is tmux's escape for a literal `#`, occupying one. This
//! replays that reading before applying `unicode-width`, mirroring
//! `visible_width` in `src/render/mod.rs` so the tests measure what tmux would
//! actually draw.

use std::iter::Peekable;
use std::str::Chars;

use unicode_width::UnicodeWidthChar;

/// The number of terminal columns a rendered tmux status line occupies.
pub fn visible_width(value: &str) -> usize {
    let mut width = 0;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '#' {
            width += UnicodeWidthChar::width(ch).unwrap_or(0);
            continue;
        }
        match chars.peek() {
            // `##` renders as a single literal `#`.
            Some('#') => {
                chars.next();
                width += 1;
            }
            // A `#[...]` style tag is markup and draws nothing.
            Some('[') => skip_style_tag(&mut chars),
            // A bare trailing `#` is drawn as itself.
            _ => width += 1,
        }
    }
    width
}

fn skip_style_tag(chars: &mut Peekable<Chars<'_>>) {
    for next in chars.by_ref() {
        if next == ']' {
            break;
        }
    }
}
