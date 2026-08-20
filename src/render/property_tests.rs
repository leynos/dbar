//! Property-based coverage for renderer escaping and width accounting.
//!
//! Split from [`super::tests`] to stay under the module line cap. These cases
//! push generated hostile values through every dynamic slot of the status
//! line, so a segment that forgets [`escape_tmux`] is caught wherever it is.

use super::tests::{DynamicValues, dynamic_value, render_dynamic, scan_constructs};
use super::*;
use proptest::prelude::*;
use unicode_width::UnicodeWidthStr;

proptest! {
    // Bounded and deterministic for continuous integration; regression files
    // are disabled because the repository tracks none.
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// No dynamic value may introduce an active `#{` or `#(` construct, while
    /// the renderer's own `#[...]` style tags survive unescaped.
    #[test]
    fn dynamic_values_never_introduce_active_constructs(
        project in dynamic_value(),
        branch in dynamic_value(),
        pr in dynamic_value(),
        clock in dynamic_value(),
        session in dynamic_value(),
        window in dynamic_value(),
        pane in dynamic_value(),
        socket in dynamic_value(),
    ) {
        let line = render_dynamic(&DynamicValues {
            project,
            branch,
            pr,
            clock,
            session,
            window,
            pane,
            socket,
        });
        let scan = scan_constructs(&line);
        prop_assert!(scan.active.is_none(), "active construct survived in: {line}");
        prop_assert!(
            scan.foreign.is_none(),
            "a value-derived style tag survived in: {line}"
        );
        prop_assert!(scan.styles > 0, "renderer style tags were escaped away: {line}");
    }

    /// Escaping preserves the value's visible width: `##` renders as one `#`.
    #[test]
    fn escaping_preserves_visible_width(value in dynamic_value()) {
        prop_assert_eq!(
            visible_width(&escape_tmux(&value)),
            UnicodeWidthStr::width(value.as_str())
        );
    }

    /// Width accounting is exact enough to pad a line to the client width.
    #[test]
    fn layout_pads_escaped_values_to_the_client_width(
        left in dynamic_value(),
        right in dynamic_value(),
        slack in 1_usize..40,
    ) {
        let escaped_left = escape_tmux(&left);
        let escaped_right = escape_tmux(&right);
        let width = visible_width(&escaped_left) + visible_width(&escaped_right) + slack + 1;
        let output = layout_with_width(&escaped_left, &escaped_right, width);
        prop_assert_eq!(visible_width(&output), width);
    }
}
