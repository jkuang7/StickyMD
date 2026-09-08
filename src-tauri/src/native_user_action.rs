use anyhow::Result;
use serde::{ser::SerializeStruct, Serialize, Serializer};

use crate::windows::Direction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeUserAction {
    Close,
    Relink,
    SetColor { color: String },
    ChangeFontSize { increase: bool },
    Snap { direction: Direction, partial: bool },
}

impl NativeUserAction {
    fn requires_note(&self) -> bool {
        matches!(
            self,
            Self::SetColor { .. } | Self::ChangeFontSize { .. } | Self::Snap { .. }
        )
    }
}

impl Serialize for NativeUserAction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Close => serializer.serialize_str("close"),
            Self::Relink => serializer.serialize_str("relink"),
            Self::SetColor { color } => {
                let mut state = serializer.serialize_struct("UserAction", 2)?;
                state.serialize_field("type", "set-color")?;
                state.serialize_field("color", color)?;
                state.end()
            }
            Self::ChangeFontSize { increase } => {
                let mut state = serializer.serialize_struct("UserAction", 2)?;
                state.serialize_field("type", "change-font-size")?;
                state.serialize_field("increase", increase)?;
                state.end()
            }
            Self::Snap { direction, partial } => {
                let mut state = serializer.serialize_struct("UserAction", 3)?;
                state.serialize_field("type", "snap")?;
                state.serialize_field("direction", direction)?;
                state.serialize_field("partial", partial)?;
                state.end()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceKind {
    Note,
    Timer,
    Utility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FocusedSurface {
    pub(crate) label: String,
    pub(crate) kind: SurfaceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
#[allow(dead_code)] // Mirrors every webview outcome even though native dispatch only creates failures.
pub(crate) enum UserActionOutcome {
    Succeeded,
    Cancelled,
    Busy { message: String },
    Failed { message: String },
}

pub(crate) trait NativeUserActionTransport {
    fn resolve_focused(&mut self) -> Result<FocusedSurface>;
    fn emit(&mut self, target: &FocusedSurface, action: &NativeUserAction) -> Result<()>;
    fn close_utility(&mut self, target: &FocusedSurface) -> Result<()>;
    fn render(&mut self, outcome: &UserActionOutcome);
}

fn failed(
    transport: &mut impl NativeUserActionTransport,
    error: anyhow::Error,
) -> UserActionOutcome {
    let outcome = UserActionOutcome::Failed {
        message: format!("{error:#}"),
    };
    transport.render(&outcome);
    outcome
}

pub(crate) fn dispatch_native_user_action(
    transport: &mut impl NativeUserActionTransport,
    action: NativeUserAction,
) -> UserActionOutcome {
    let target = match transport.resolve_focused() {
        Ok(target) => target,
        Err(error) => return failed(transport, error),
    };

    if target.kind == SurfaceKind::Utility {
        if action == NativeUserAction::Close {
            return match transport.close_utility(&target) {
                Ok(()) => UserActionOutcome::Succeeded,
                Err(error) => failed(transport, error),
            };
        }
        return failed(
            transport,
            anyhow::anyhow!("The focused surface is not a note or timer"),
        );
    }

    if action.requires_note() && target.kind != SurfaceKind::Note {
        return failed(
            transport,
            anyhow::anyhow!("The focused surface is not a note"),
        );
    }

    match transport.emit(&target, &action) {
        Ok(()) => UserActionOutcome::Succeeded,
        Err(error) => failed(transport, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{bail, Result};

    #[derive(Default)]
    struct MemoryTransport {
        focused: Option<FocusedSurface>,
        resolve_error: bool,
        emit_error: bool,
        emitted: Vec<(String, NativeUserAction)>,
        closed_utilities: Vec<String>,
        rendered: Vec<UserActionOutcome>,
        resolutions: usize,
    }

    impl NativeUserActionTransport for MemoryTransport {
        fn resolve_focused(&mut self) -> Result<FocusedSurface> {
            self.resolutions += 1;
            if self.resolve_error {
                bail!("focus inspection failed");
            }
            self.focused
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No window is currently focused"))
        }

        fn emit(&mut self, target: &FocusedSurface, action: &NativeUserAction) -> Result<()> {
            if self.emit_error {
                bail!("event delivery failed");
            }
            self.emitted.push((target.label.clone(), action.clone()));
            Ok(())
        }

        fn close_utility(&mut self, target: &FocusedSurface) -> Result<()> {
            self.closed_utilities.push(target.label.clone());
            Ok(())
        }

        fn render(&mut self, outcome: &UserActionOutcome) {
            self.rendered.push(outcome.clone());
        }
    }

    fn focused(label: &str, kind: SurfaceKind) -> FocusedSurface {
        FocusedSurface {
            label: label.into(),
            kind,
        }
    }

    #[test]
    fn shared_actions_resolve_once_and_emit_the_exact_production_contract() {
        let mut transport = MemoryTransport {
            focused: Some(focused("sticky_a", SurfaceKind::Note)),
            ..MemoryTransport::default()
        };

        assert_eq!(
            dispatch_native_user_action(&mut transport, NativeUserAction::Relink),
            UserActionOutcome::Succeeded
        );
        assert_eq!(transport.resolutions, 1);
        assert_eq!(
            transport.emitted,
            [("sticky_a".into(), NativeUserAction::Relink)]
        );
        assert!(transport.rendered.is_empty());
        assert_eq!(
            serde_json::to_value(NativeUserAction::Close).unwrap(),
            serde_json::json!("close")
        );
        assert_eq!(
            serde_json::to_value(NativeUserAction::Snap {
                direction: crate::windows::Direction::Left,
                partial: true,
            })
            .unwrap(),
            serde_json::json!({"type": "snap", "direction": "Left", "partial": true})
        );
        assert_eq!(
            serde_json::to_value(UserActionOutcome::Cancelled).unwrap(),
            serde_json::json!({"status": "cancelled"})
        );
        assert_eq!(
            serde_json::to_value(UserActionOutcome::Busy {
                message: "already running".into(),
            })
            .unwrap(),
            serde_json::json!({"status": "busy", "message": "already running"})
        );
    }

    #[test]
    fn every_early_failure_is_normalized_rendered_and_mutation_free() {
        let cases = [
            MemoryTransport::default(),
            MemoryTransport {
                resolve_error: true,
                ..MemoryTransport::default()
            },
            MemoryTransport {
                focused: Some(focused("timer_a", SurfaceKind::Timer)),
                ..MemoryTransport::default()
            },
            MemoryTransport {
                focused: Some(focused("keyboard_shortcuts", SurfaceKind::Utility)),
                ..MemoryTransport::default()
            },
            MemoryTransport {
                focused: Some(focused("sticky_a", SurfaceKind::Note)),
                emit_error: true,
                ..MemoryTransport::default()
            },
        ];

        for mut transport in cases {
            let outcome = dispatch_native_user_action(
                &mut transport,
                NativeUserAction::SetColor {
                    color: "#fff9b1".into(),
                },
            );
            assert!(matches!(outcome, UserActionOutcome::Failed { .. }));
            assert_eq!(transport.resolutions, 1);
            assert!(transport.emitted.is_empty());
            assert!(transport.closed_utilities.is_empty());
            assert_eq!(transport.rendered, [outcome]);
        }
    }

    #[test]
    fn transport_failure_keeps_the_resolved_target_stable_and_visible() {
        let mut transport = MemoryTransport {
            focused: Some(focused("sticky_first", SurfaceKind::Note)),
            emit_error: true,
            ..MemoryTransport::default()
        };

        let outcome = dispatch_native_user_action(&mut transport, NativeUserAction::Close);

        assert_eq!(transport.resolutions, 1);
        assert_eq!(
            outcome,
            UserActionOutcome::Failed {
                message: "event delivery failed".into()
            }
        );
        assert_eq!(transport.rendered, [outcome]);
    }

    #[test]
    fn utility_close_preserves_utility_behavior_without_surface_fallback() {
        let mut transport = MemoryTransport {
            focused: Some(focused("version", SurfaceKind::Utility)),
            ..MemoryTransport::default()
        };

        assert_eq!(
            dispatch_native_user_action(&mut transport, NativeUserAction::Close),
            UserActionOutcome::Succeeded
        );
        assert_eq!(transport.resolutions, 1);
        assert_eq!(transport.closed_utilities, ["version"]);
        assert!(transport.emitted.is_empty());
    }
}
