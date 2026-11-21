//! The columns of a team board, and how they map onto task statuses.
//!
//! A team board is the team's slice of the shared task queue (`task_queue`
//! rows with that `team_id`). The board shows four columns; each holds one
//! status, so moving a card is changing its status and nothing else:
//!
//! | column   | status        | who puts it there                         |
//! |----------|---------------|-------------------------------------------|
//! | `todo`   | `pending`     | posting a task; dragging it back          |
//! | `doing`  | `in_progress` | an agent's `task_claim`; a person's drag  |
//! | `review` | `review`      | `task_complete` with `review`; a drag     |
//! | `done`   | `completed`   | `task_complete`; a drag                   |
//!
//! `review` is the only status the queue did not have before. Nothing claims
//! a task in review, so work handed in for review waits for a person or a
//! lead instead of being picked up again.

/// The board's columns in order, each with the status it holds.
pub const BOARD_COLUMNS: [(&str, &str); 4] = [
    ("todo", "pending"),
    ("doing", "in_progress"),
    ("review", "review"),
    ("done", "completed"),
];

/// The status for a column name or a status name, case-insensitive;
/// `None` for anything else.
pub fn board_status(column_or_status: &str) -> Option<&'static str> {
    let key = column_or_status.trim();
    BOARD_COLUMNS
        .iter()
        .find(|(c, s)| c.eq_ignore_ascii_case(key) || s.eq_ignore_ascii_case(key))
        .map(|(_, s)| *s)
}

/// The column a status is shown in. An unknown status (a row written by
/// something else) shows in `todo`, where someone will see it.
pub fn board_column(status: &str) -> &'static str {
    BOARD_COLUMNS
        .iter()
        .find(|(_, s)| *s == status)
        .map(|(c, _)| *c)
        .unwrap_or("todo")
}

/// A change to one card. `None` leaves a field as it is; an empty
/// `assigned_to` makes the card anyone's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskChange {
    pub title: Option<String>,
    pub description: Option<String>,
    /// An agent id, `user:<name>` for a person, or empty for anyone.
    pub assigned_to: Option<String>,
    /// A column (`todo`, `doing`, `review`, `done`) or the status it holds.
    pub status: Option<String>,
}

/// The `assigned_to` value that names a person rather than an agent.
///
/// Agents are stored by id; a person by `user:<name>`. `task_claim` only ever
/// matches an agent's own id or an empty assignee, so a card assigned to a
/// person is never picked up by an agent.
pub fn person_assignee(username: &str) -> String {
    format!("user:{}", username.trim())
}

/// The person named by an `assigned_to` value, if it names one.
pub fn assignee_person(assigned_to: &str) -> Option<&str> {
    assigned_to
        .strip_prefix("user:")
        .map(str::trim)
        .filter(|u| !u.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_statuses_map_both_ways() {
        assert_eq!(board_status("todo"), Some("pending"));
        assert_eq!(board_status("Doing"), Some("in_progress"));
        assert_eq!(board_status("review"), Some("review"));
        assert_eq!(board_status("done"), Some("completed"));
        assert_eq!(board_status("completed"), Some("completed"));
        assert_eq!(board_status("in_progress"), Some("in_progress"));
        assert_eq!(board_status("archived"), None);
        for (column, status) in BOARD_COLUMNS {
            assert_eq!(board_column(status), column);
        }
        assert_eq!(board_column("something-else"), "todo");
    }

    #[test]
    fn people_are_told_apart_from_agents() {
        assert_eq!(person_assignee(" uma "), "user:uma");
        assert_eq!(assignee_person("user:uma"), Some("uma"));
        assert_eq!(assignee_person("user:"), None);
        assert_eq!(assignee_person("6f1c0a3e-agent"), None);
    }
}
