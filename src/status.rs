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
            Self::Idle => "idle",
            Self::Found => "found",
            Self::Unknown => "unknown",
            Self::Ended => "ended",
        }
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
            "stop" | "afterAgentResponse" => Self::Done,
            "sessionEnd" => Self::Ended,
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
    }

    #[test]
    fn ignores_unknown_and_claude_only_events() {
        assert_eq!(Status::from_cursor_hook("Notification"), None);
        assert_eq!(Status::from_cursor_hook("PermissionRequest"), None);
        assert_eq!(Status::from_cursor_hook(""), None);
    }
}
