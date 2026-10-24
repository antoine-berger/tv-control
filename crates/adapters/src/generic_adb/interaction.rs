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
}

/// Percent is rejected because Android rewrites `%s` to a space before injecting
/// key events. Shell quoting alone cannot preserve arbitrary input text semantics.
fn validate_input_text(text: &str) -> PortResult<()> {
    if text.trim().is_empty() || text.len() > 500 {
        return Err(Failure::before("invalid_input_text"));
    }
    if !text
        .bytes()
        .all(|b| (b' '..=b'~').contains(&b) && b != b'%')
    {
        return Err(Failure::before("input_text_unsupported"));
    }
    Ok(())
}
