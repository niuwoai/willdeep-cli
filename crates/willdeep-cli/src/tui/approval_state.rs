use super::*;

impl App {
    /// Show an approval immediately, or queue it behind the one on screen.
    /// Returns true when the request became visible right now — the caller
    /// rings the terminal bell for that case, so a user looking elsewhere
    /// learns the turn is parked instead of watching it appear to hang.
    pub(super) fn enqueue_approval(&mut self, request: ApprovalRequest) -> bool {
        let waiting = self
            .language
            .text("等待你确认", "Waiting for you", "確認待ち");
        self.record_progress(format!("{waiting} · {}", first_line(&request.0)));
        if self.approval.is_some() {
            self.approval_queue.push_back(request);
            return false;
        }
        self.approval = Some(request);
        self.approval_action_hits.clear();
        self.approval_rect = Rect::default();
        self.approval_view = human_gate_ui::ApprovalViewport::default();
        self.qterm_gate_generation = self.qterm_gate_generation.wrapping_add(1);
        self.approval_selected = 0;
        true
    }

    /// Approval dialogs must remain usable while an IME owns alphabetic
    /// keystrokes. Unknown keys are consumed without deciding anything;
    /// Enter confirms the selected row and Esc explicitly denies.
    pub(super) fn handle_approval_key(&mut self, key: KeyEvent) {
        let Some((_, always, _)) = self.approval.as_ref() else {
            return;
        };
        let always = *always;
        let decisions = approval_decisions(always);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT);
        let view = &mut self.approval_view;
        let scrollable = view.total_lines > usize::from(view.body.height);
        let decision = match key.code {
            KeyCode::Up | KeyCode::Left if plain => {
                self.approval_selected = self
                    .approval_selected
                    .checked_sub(1)
                    .unwrap_or(decisions.len() - 1);
                None
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Tab if plain => {
                self.approval_selected = (self.approval_selected + 1) % decisions.len();
                None
            }
            KeyCode::BackTab if plain => {
                self.approval_selected = self
                    .approval_selected
                    .checked_sub(1)
                    .unwrap_or(decisions.len() - 1);
                None
            }
            KeyCode::PageUp if plain && scrollable => {
                view.scroll_by(-1);
                None
            }
            KeyCode::PageDown if plain && scrollable => {
                view.scroll_by(1);
                None
            }
            KeyCode::Home if plain && scrollable => {
                view.offset = 0;
                None
            }
            KeyCode::End if plain && scrollable => {
                view.offset = view
                    .total_lines
                    .saturating_sub(usize::from(view.body.height));
                None
            }
            KeyCode::Enter if plain => {
                decisions
                    .get(self.approval_selected)
                    .copied()
                    .and_then(|selected| {
                        if human_gate_ui::approval_decision_blocked(selected, view) {
                            None
                        } else {
                            Some(selected)
                        }
                    })
            }
            KeyCode::Esc => Some(ApprovalDecision::Deny),
            KeyCode::Char('y' | 'Y' | '是') if plain => {
                human_gate_ui::approval_allow_if_read(view, ApprovalDecision::AllowOnce)
            }
            KeyCode::Char('a' | 'A') if plain && always => {
                human_gate_ui::approval_allow_if_read(view, ApprovalDecision::AlwaysAllow)
            }
            KeyCode::Char('n' | 'N' | '否') if plain => Some(ApprovalDecision::Deny),
            _ => None,
        };
        if let Some(decision) = decision {
            self.resolve_approval(|_| decision);
        }
    }

    /// Some terminals surface committed IME text as a paste event. Accept
    /// only an exact approval word and never leak it into the prompt behind
    /// the modal.
    pub(super) fn handle_approval_text(&mut self, value: &str) {
        let Some((_, always, _)) = self.approval.as_ref() else {
            return;
        };
        let normalized = value.trim().to_lowercase();
        let view = &self.approval_view;
        let decision = match normalized.as_str() {
            "y" | "yes" | "是" | "允许" | "同意" => {
                human_gate_ui::approval_allow_if_read(view, ApprovalDecision::AllowOnce)
            }
            "a" | "always" | "始终允许" if *always => {
                human_gate_ui::approval_allow_if_read(view, ApprovalDecision::AlwaysAllow)
            }
            "n" | "no" | "否" | "拒绝" => Some(ApprovalDecision::Deny),
            _ => None,
        };
        if let Some(decision) = decision {
            self.resolve_approval(|_| decision);
        }
    }

    /// Answer the visible approval and immediately promote the next queued
    /// one, so a turn that needs three confirmations asks three times in a
    /// row instead of stalling after the first.
    pub(super) fn resolve_approval(&mut self, decide: impl FnOnce(bool) -> ApprovalDecision) {
        let Some((_, always, sender)) = self.approval.take() else {
            return;
        };
        let _ = sender.send(decide(always));
        self.approval = self.approval_queue.pop_front();
        self.approval_action_hits.clear();
        self.approval_rect = Rect::default();
        self.approval_view = human_gate_ui::ApprovalViewport::default();
        self.qterm_gate_generation = self.qterm_gate_generation.wrapping_add(1);
        self.approval_selected = 0;
    }
}
