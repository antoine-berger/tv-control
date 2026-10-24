//! Single-use UI references, bounded capture and validated text/key dispatch.
use super::*;

impl GenericAdb {
    pub(super) fn read_ui(&self, alias: &str, ctx: &Context<'_>) -> PortResult<UiSnapshot> {
        let raw = self.shell_bytes(
            alias,
            // Some Android builds cannot write the hierarchy through /proc/self/fd/1.
            // Use a private, unique temporary file and clean it up on shell exit.
            // This fixed script contains no device- or caller-supplied shell text.
            &[
                "sh",
                "-c",
                "umask 077; f=$(mktemp /data/local/tmp/tv-control-ui.XXXXXX) || exit 1; trap 'rm -f \"$f\"' EXIT; uiautomator dump --compressed \"$f\" >/dev/null && cat \"$f\"",
            ],
            &[],
            false,
            512 * 1024,
            ctx,
        )?;
        let raw = String::from_utf8(raw).map_err(|_| Failure::before("ui_unavailable"))?;
        parse_ui(alias, self.session, &raw)
    }
    pub(super) fn issue_ui(&self, alias: &str, ctx: &Context<'_>) -> PortResult<UiSnapshot> {
        let mut snapshot = self.read_ui(alias, ctx)?;
        let sequence = self.view_sequence.fetch_add(1, Ordering::Relaxed);
        for (index, element) in snapshot.elements.iter_mut().enumerate() {
            element.reference = format!("{}:{sequence}:{index}", snapshot.revision);
        }
        self.views
            .lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?
            .insert(
                alias.to_owned(),
                IssuedView {
                    snapshot: snapshot.clone(),
                    at: ctx.clock.monotonic_ms(),
                },
            );
        Ok(snapshot)
    }
    pub(super) fn interact(
        &self,
        alias: &str,
        intent: &Intent,
        reference: &str,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        // Android's input text is key injection, not a Unicode clipboard.
        // Validate before reading or consuming the caller's snapshot.
        if let Intent::TypeText { text, .. } = intent {
            validate_input_text(text)?;
        }
        let (snapshot, at) = {
            let views = self
                .views
                .lock()
                .map_err(|_| Failure::before("adapter_state_unavailable"))?;
            let view = views
                .get(alias)
                .ok_or(Failure::before("ui_reference_unknown"))?;
            (view.snapshot.clone(), view.at)
        };
        if ctx.clock.monotonic_ms().saturating_sub(at) > REFERENCE_TTL_MS {
            return Err(Failure::before("ui_reference_expired"));
        }
        let element = snapshot
            .elements
            .iter()
            .find(|e| e.reference == reference)
            .ok_or(Failure::before("ui_reference_unknown"))?;
        match intent {
            Intent::TypeText { .. }
                if !element.editable || !element.focused || snapshot.focused_count != 1 =>
            {
                return Err(Failure::before("text_focus_not_editable"));
            }
            Intent::Activate { .. } if !element.actionable => {
                return Err(Failure::before("ui_element_not_actionable"));
            }
            _ => {}
        }
        let expected_text = if let Intent::TypeText { text, .. } = intent {
            Some(expected_accessibility_text(&snapshot, element, text)?)
        } else {
            None
        };
        let fresh = self.read_ui(alias, ctx)?;
        // Revision includes the entire raw hierarchy, device and adapter
        // session. Issuance-specific refs are intentionally not compared.
        if fresh.revision != snapshot.revision {
            return Err(Failure::before("ui_reference_stale"));
        }
        if matches!(intent, Intent::TypeText { .. }) {
            let package = element
                .package
                .as_ref()
                .ok_or(Failure::before("text_focus_app_unknown"))?;
            if self.foreground(alias, ctx)?.as_ref() != Some(package) {
                return Err(Failure::before("text_focus_app_changed"));
            }
        }
        // Dumping can be slow: enforce the TTL again immediately before dispatch.
        if ctx.clock.monotonic_ms().saturating_sub(at) > REFERENCE_TTL_MS {
            return Err(Failure::before("ui_reference_expired"));
        }
        // Even an uncertain mutation consumes references; never replay it.
        {
            let mut views = self
                .views
                .lock()
                .map_err(|_| Failure::before("adapter_state_unavailable"))?;
            if !views.get(alias).is_some_and(|view| {
                view.snapshot
                    .elements
                    .iter()
                    .any(|e| e.reference == reference)
            }) {
                return Err(Failure::before("ui_reference_unknown"));
            }
            views.remove(alias);
        }
        if let Intent::TypeText { text, .. } = intent {
            self.shell(alias, &["input", "text", text], &[], true, ctx)?;
            // Shell success proves key dispatch, not even matching accessibility text.
            // Readback remains UI evidence only, not proof of underlying committed text.
            // One post-read only: never repeat or try to repair uncertain input.
            self.verify_accessibility_text(alias, element, expected_text.as_deref().unwrap(), ctx)?;
        } else {
            let [left, top, right, bottom] = element.bounds;
            let x = left + (right - left) / 2;
            let y = top + (bottom - top) / 2;
            self.shell(
                alias,
                &["input", "tap", &x.to_string(), &y.to_string()],
                &[],
                true,
                ctx,
            )?;
        }
        Ok(())
    }

    fn verify_accessibility_text(
        &self,
        alias: &str,
        before: &UiElement,
        expected: &str,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        let verify = || {
            ctx.check()?;
            let after = self.read_ui(alias, ctx)?;
            ctx.check()?;
            if self.foreground(alias, ctx)? != before.package {
                return Err(Failure::before("text_result_context_changed"));
            }
            ctx.check()?;
            let editors: Vec<_> = after
                .elements
                .iter()
                .filter(|e| same_editor(before, e))
                .collect();
            if after.focused_count != 1
                || !after.truncation.is_empty()
                || editors.len() != 1
                || !editors[0].focused
                || !editors[0].editable
            {
                return Err(Failure::before("text_result_context_changed"));
            }
            let actual = editors[0]
                .accessibility_text
                .as_deref()
                .ok_or(Failure::before("text_result_unreadable"))?;
            if actual != expected {
                return Err(Failure::before("text_result_mismatch"));
            }
            Ok(())
        };
        verify().map_err(|e| Failure::uncertain(e.code))
    }
}

fn same_editor(a: &UiElement, b: &UiElement) -> bool {
    a.resource_id == b.resource_id
        && a.package == b.package
        && a.role == b.role
        && a.bounds == b.bounds
}

fn expected_accessibility_text(
    snapshot: &UiSnapshot,
    editor: &UiElement,
    text: &str,
) -> PortResult<String> {
    let unsupported = || Failure::before("text_verification_unsupported");
    let before = editor
        .accessibility_text
        .as_deref()
        .ok_or_else(unsupported)?;
    if editor.resource_id.is_none()
        || !snapshot.truncation.is_empty()
        || snapshot
            .elements
            .iter()
            .filter(|e| same_editor(editor, e))
            .count()
            != 1
        || before.chars().count() + text.chars().count() > 256
    {
        return Err(unsupported());
    }
    Ok(format!("{before}{text}"))
}

/// Percent is rejected because Android rewrites `%s` to a space before injecting
/// key events. Question mark is ambiguous with UIAutomator sanitization of Unicode
/// and controls. Shell quoting cannot preserve arbitrary input text semantics.
fn validate_input_text(text: &str) -> PortResult<()> {
    if text.trim().is_empty() || text.len() > 500 {
        return Err(Failure::before("invalid_input_text"));
    }
    if !text
        .bytes()
        .all(|b| (b' '..=b'~').contains(&b) && b != b'%' && b != b'?')
    {
        return Err(Failure::before("input_text_unsupported"));
    }
    Ok(())
}
