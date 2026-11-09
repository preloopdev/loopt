//! Static GitHub Actions trigger data.
//!
//! These tables mirror the reference pages below and are cross-checked against
//! actionlint's generated `all_webhooks.go` and its filter table in
//! `rule_events.go`:
//!
//! - <https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows>
//! - <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax>
//! - <https://github.com/rhysd/actionlint/blob/main/all_webhooks.go>
//! - <https://github.com/rhysd/actionlint/blob/main/rule_events.go>

/// Activity types accepted by a workflow event.
#[derive(Clone, Copy)]
pub(crate) enum ActivityTypes {
    /// Any string is accepted; `repository_dispatch` event types are
    /// user-defined and cannot be enumerated.
    Any,
    /// Only the listed values are accepted. An empty list means the event does
    /// not support `types:` at all.
    Listed(&'static [&'static str]),
}

/// All workflow events and their activity types.
pub(crate) static EVENTS: &[(&str, ActivityTypes)] = &[
    (
        "branch_protection_rule",
        ActivityTypes::Listed(&["created", "edited", "deleted"]),
    ),
    (
        "check_run",
        ActivityTypes::Listed(&["created", "rerequested", "completed", "requested_action"]),
    ),
    ("check_suite", ActivityTypes::Listed(&["completed"])),
    ("create", ActivityTypes::Listed(&[])),
    ("delete", ActivityTypes::Listed(&[])),
    ("deployment", ActivityTypes::Listed(&[])),
    ("deployment_status", ActivityTypes::Listed(&[])),
    (
        "discussion",
        ActivityTypes::Listed(&[
            "created",
            "edited",
            "deleted",
            "transferred",
            "pinned",
            "unpinned",
            "labeled",
            "unlabeled",
            "locked",
            "unlocked",
            "category_changed",
            "answered",
            "unanswered",
        ]),
    ),
    (
        "discussion_comment",
        ActivityTypes::Listed(&["created", "edited", "deleted"]),
    ),
    ("fork", ActivityTypes::Listed(&[])),
    ("gollum", ActivityTypes::Listed(&[])),
    ("image_version", ActivityTypes::Listed(&[])),
    (
        "issue_comment",
        ActivityTypes::Listed(&["created", "edited", "deleted"]),
    ),
    (
        "issues",
        ActivityTypes::Listed(&[
            "opened",
            "edited",
            "deleted",
            "transferred",
            "pinned",
            "unpinned",
            "closed",
            "reopened",
            "assigned",
            "unassigned",
            "labeled",
            "unlabeled",
            "locked",
            "unlocked",
            "milestoned",
            "demilestoned",
            "typed",
            "untyped",
        ]),
    ),
    (
        "label",
        ActivityTypes::Listed(&["created", "edited", "deleted"]),
    ),
    ("merge_group", ActivityTypes::Listed(&["checks_requested"])),
    (
        "milestone",
        ActivityTypes::Listed(&["created", "closed", "opened", "edited", "deleted"]),
    ),
    ("page_build", ActivityTypes::Listed(&[])),
    ("public", ActivityTypes::Listed(&[])),
    (
        "pull_request",
        ActivityTypes::Listed(&[
            "assigned",
            "unassigned",
            "labeled",
            "unlabeled",
            "opened",
            "edited",
            "closed",
            "reopened",
            "synchronize",
            "converted_to_draft",
            "locked",
            "unlocked",
            "enqueued",
            "dequeued",
            "milestoned",
            "demilestoned",
            "ready_for_review",
            "review_requested",
            "review_request_removed",
            "auto_merge_enabled",
            "auto_merge_disabled",
        ]),
    ),
    (
        "pull_request_review",
        ActivityTypes::Listed(&["submitted", "edited", "dismissed"]),
    ),
    (
        "pull_request_review_comment",
        ActivityTypes::Listed(&["created", "edited", "deleted"]),
    ),
    (
        "pull_request_target",
        ActivityTypes::Listed(&[
            "assigned",
            "unassigned",
            "labeled",
            "unlabeled",
            "opened",
            "edited",
            "closed",
            "reopened",
            "synchronize",
            "converted_to_draft",
            "locked",
            "unlocked",
            "enqueued",
            "dequeued",
            "milestoned",
            "demilestoned",
            "ready_for_review",
            "review_requested",
            "review_request_removed",
            "auto_merge_enabled",
            "auto_merge_disabled",
        ]),
    ),
    ("push", ActivityTypes::Listed(&[])),
    (
        "registry_package",
        ActivityTypes::Listed(&["published", "updated"]),
    ),
    (
        "release",
        ActivityTypes::Listed(&[
            "published",
            "unpublished",
            "created",
            "edited",
            "deleted",
            "prereleased",
            "released",
        ]),
    ),
    ("repository_dispatch", ActivityTypes::Any),
    ("schedule", ActivityTypes::Listed(&[])),
    ("status", ActivityTypes::Listed(&[])),
    ("watch", ActivityTypes::Listed(&["started"])),
    ("workflow_call", ActivityTypes::Listed(&[])),
    ("workflow_dispatch", ActivityTypes::Listed(&[])),
    (
        "workflow_run",
        ActivityTypes::Listed(&["completed", "requested", "in_progress"]),
    ),
];

/// Events that accept `branches` / `branches-ignore`.
pub(crate) const BRANCH_FILTER_EVENTS: &[&str] = &[
    "merge_group",
    "pull_request",
    "pull_request_target",
    "push",
    "workflow_run",
];

/// Events that accept `paths` / `paths-ignore`.
pub(crate) const PATH_FILTER_EVENTS: &[&str] = &["pull_request", "pull_request_target", "push"];

/// Events that accept `tags` / `tags-ignore`.
pub(crate) const TAG_FILTER_EVENTS: &[&str] = &["push"];

/// Input types accepted by `on.workflow_dispatch.inputs.<id>.type`.
pub(crate) const DISPATCH_INPUT_TYPES: &[&str] =
    &["boolean", "choice", "environment", "number", "string"];

/// Input types accepted by `on.workflow_call.inputs.<id>.type`.
pub(crate) const CALL_INPUT_TYPES: &[&str] = &["boolean", "number", "string"];

/// Look up an event's activity-type rules; `None` means the event is unknown.
pub(crate) fn activity_types(event: &str) -> Option<ActivityTypes> {
    EVENTS
        .iter()
        .find(|(name, _)| *name == event)
        .map(|(_, types)| *types)
}

/// Whether `event` supports the given filter family.
pub(crate) fn supports_branch_filter(event: &str) -> bool {
    BRANCH_FILTER_EVENTS.contains(&event)
}

pub(crate) fn supports_path_filter(event: &str) -> bool {
    PATH_FILTER_EVENTS.contains(&event)
}

pub(crate) fn supports_tag_filter(event: &str) -> bool {
    TAG_FILTER_EVENTS.contains(&event)
}
