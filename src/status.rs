//! Board statuses. Hook event names stay Cursor-shaped; the adapter maps host
//! payloads into these variants.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Failed,
    Waiting,
    IdleWait,
    Done,
    Compact,
    Working,
    Interrupted,
    Idle,
    Found,
    Unknown,
    Ended,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Waiting => "waiting",
            Self::IdleWait => "idle-wait",
            Self::Done => "done",
            Self::Compact => "compact",
            Self::Working => "working",
            Self::Interrupted => "stopped",
            Self::Idle => "idle",
            Self::Found => "found",
            Self::Unknown => "unknown",
            Self::Ended => "ended",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Failed => "✗",
            Self::Interrupted => "■",
            Self::Waiting | Self::Working => "●",
            Self::IdleWait => "◑",
            Self::Done => "✓",
            Self::Compact => "◐",
            Self::Idle => "○",
            Self::Found => "◌",
            Self::Unknown | Self::Ended => "?",
        }
    }

    /// Zellij theme slots 0–3, same mapping as zj-agent-mob.
    pub fn color_level(self) -> usize {
        match self {
            Self::Waiting | Self::IdleWait => 2,
            Self::Working | Self::Compact => 0,
            Self::Done => 1,
            Self::Idle
            | Self::Failed
            | Self::Found
            | Self::Unknown
            | Self::Ended
            | Self::Interrupted => 3,
        }
    }

    pub fn is_error(self) -> bool {
        matches!(self, Self::Failed)
    }

    /// Cursor `hook_event_name` → status. Unknown names are ignored so a new
    /// hook cannot invent a row or flip state by accident.
    pub fn from_cursor_hook(event: &str) -> Option<Self> {
        Some(match event {
            "sessionStart" => Self::Idle,
            "beforeSubmitPrompt" => Self::Working,
            "preToolUse" | "postToolUse" | "beforeShellExecution" | "afterShellExecution" => {
                Self::Working
            }
            "preCompact" => Self::Compact,
            "postCompact" => Self::Working,
            "stop" | "afterAgentResponse" => Self::Done,
            "sessionEnd" => Self::Ended,
            "interrupt" => Self::Interrupted,
            "permissionRequest" => Self::Waiting,
            "idleWait" => Self::IdleWait,
            "stopFailure" => Self::Failed,
            "postToolUseFailure" => Self::Working,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Status;

    #[test]
    fn maps_the_documented_cursor_hooks() {
        assert_eq!(Status::from_cursor_hook("sessionStart"), Some(Status::Idle));
        assert_eq!(
            Status::from_cursor_hook("beforeSubmitPrompt"),
            Some(Status::Working)
        );
        assert_eq!(
            Status::from_cursor_hook("preToolUse"),
            Some(Status::Working)
        );
        assert_eq!(
            Status::from_cursor_hook("preCompact"),
            Some(Status::Compact)
        );
        assert_eq!(Status::from_cursor_hook("stop"), Some(Status::Done));
        assert_eq!(Status::from_cursor_hook("sessionEnd"), Some(Status::Ended));
        assert_eq!(
            Status::from_cursor_hook("interrupt"),
            Some(Status::Interrupted)
        );
        assert_eq!(
            Status::from_cursor_hook("permissionRequest"),
            Some(Status::Waiting)
        );
        assert_eq!(Status::from_cursor_hook("idleWait"), Some(Status::IdleWait));
        assert_eq!(
            Status::from_cursor_hook("stopFailure"),
            Some(Status::Failed)
        );
        assert_eq!(
            Status::from_cursor_hook("postCompact"),
            Some(Status::Working)
        );
        assert_eq!(Status::Working.icon(), "●");
        assert_eq!(Status::Working.color_level(), 0);
        assert!(!Status::Working.is_error());
        assert!(Status::Failed.is_error());
    }

    #[test]
    fn ignores_unmapped_raw_hook_names() {
        assert_eq!(Status::from_cursor_hook("Notification"), None);
        assert_eq!(Status::from_cursor_hook("PermissionRequest"), None);
        assert_eq!(Status::from_cursor_hook(""), None);
    }
}
