//! Display-width accounting for rendered tmux status lines.
//!
//! dbar emits tmux markup rather than ANSI escapes, so a rendered line cannot
//! be measured by character count. `#[...]` style tags are markup and occupy no
//! columns, and `##` is tmux's escape for a literal `#`, occupying one. This
//! replays that reading before applying `unicode-width`, mirroring
//! `visible_width` in `src/render/mod.rs` so the tests measure what tmux would
//! actually draw.
//!
//! # Why this duplicates the renderer rather than calling it
//!
//! The obvious alternative is to export `render::visible_width` from the crate
//! root and call it here. It is rejected on two counts.
//!
//! The crate's public API is deliberately just `run` and `DbarError`: dbar is a
//! binary with a thin library face, and every additional public item becomes a
//! semantic-versioning commitment made for a test's convenience rather than for
//! a caller. Widening that surface is a real, permanent cost.
//!
//! More importantly, it would gut the assertion it serves. The snapshot test
//! checks that a rendered line fills the requested client width exactly. If it
//! measured that line with the very function the renderer used to lay it out,
//! the two would agree by construction — a miscount in `visible_width` would
//! shift the padding and the measurement together, and the assertion would pass
//! regardless. An independent reading of tmux's rules is what makes the check
//! capable of failing, so the duplication is the point.
//!
//! The cost is that the two must be kept in step. That is bounded: tmux's
//! escaping rules are fixed and short, and a divergence surfaces immediately as
//! a failing width assertion rather than silently.

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
