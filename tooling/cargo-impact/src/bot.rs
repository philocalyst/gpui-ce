//! Stateless GitHub Actions bot control plane. It never executes PR code.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ImpactReport,
    http::{Api, ApiError},
};

#[derive(Debug, Deserialize)]
pub struct CommentEvent {
    action: String,
    repository: EventRepository,
    issue: EventIssue,
    comment: EventComment,
}
#[derive(Debug, Deserialize)]
struct EventRepository {
    full_name: String,
}
#[derive(Debug, Deserialize)]
struct EventIssue {
    number: u64,
    pull_request: Option<serde_json::Value>,
}
#[derive(Debug, Deserialize)]
struct EventComment {
    id: u64,
    body: String,
    user: EventUser,
}
#[derive(Debug, Deserialize)]
struct EventUser {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BotPlan {
    pub repository: String,
    pub pull_request: u64,
    pub comment_id: u64,
    pub requested_by: String,
    pub baseline_sha: String,
    pub candidate_sha: String,
    pub candidate_repository: String,
}

#[derive(Debug, Error)]
pub enum BotError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("event does not belong to the configured repository")]
    RepositoryMismatch,
    #[error("requester does not currently have write, maintain, or admin permission")]
    Unauthorized,
    #[error("pull request is closed or its revisions are invalid")]
    InvalidPullRequest,
}

/// `event` must come from a trusted Actions event file or a verified webhook.
/// Author association and email/account badges are deliberately insufficient.
pub fn plan(
    api: &Api,
    event: &CommentEvent,
    repository: &str,
    bot_name: &str,
) -> Result<Option<BotPlan>, BotError> {
    if event.repository.full_name != repository || !safe_repository(repository) {
        return Err(BotError::RepositoryMismatch);
    }
    let mention_command = format!("@{bot_name} check");
    let slash_command = format!("/{bot_name} check");
    if event.action != "created"
        || event.issue.pull_request.is_none()
        || ![mention_command, slash_command]
            .iter()
            .any(|command| event.comment.body.trim() == command)
        || event.comment.user.kind != "User"
        || !safe_login(&event.comment.user.login)
    {
        return Ok(None);
    }
    let permission: Permission = api.get(
        &format!(
            "repos/{repository}/collaborators/{}/permission",
            event.comment.user.login
        ),
        &[],
    )?;
    if !matches!(
        permission.permission.as_str(),
        "write" | "maintain" | "admin"
    ) {
        return Err(BotError::Unauthorized);
    }
    let pr: PullRequest = api.get(
        &format!("repos/{repository}/pulls/{}", event.issue.number),
        &[],
    )?;
    if pr.state != "open"
        || !safe_sha(&pr.base.sha)
        || !safe_sha(&pr.head.sha)
        || !safe_repository(&pr.head.repo.full_name)
    {
        return Err(BotError::InvalidPullRequest);
    }
    Ok(Some(BotPlan {
        repository: repository.into(),
        pull_request: event.issue.number,
        comment_id: event.comment.id,
        requested_by: event.comment.user.login.clone(),
        baseline_sha: pr.base.sha,
        candidate_sha: pr.head.sha,
        candidate_repository: pr.head.repo.full_name,
    }))
}

pub fn acknowledge(api: &Api, plan: &BotPlan) -> Result<(), BotError> {
    let _: serde_json::Value = api.post(
        &format!(
            "repos/{}/issues/comments/{}/reactions",
            plan.repository, plan.comment_id
        ),
        serde_json::json!({"content": "eyes"}),
    )?;
    Ok(())
}

fn safe_repository(value: &str) -> bool {
    let components: Vec<_> = value.split('/').collect();
    components.len() == 2
        && components.iter().all(|v| {
            !v.is_empty()
                && *v != "."
                && *v != ".."
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
}
fn safe_login(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}
fn safe_sha(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(Deserialize)]
struct Permission {
    permission: String,
}
#[derive(Deserialize)]
struct PullRequest {
    state: String,
    base: Revision,
    head: Revision,
}
#[derive(Deserialize)]
struct Revision {
    sha: String,
    repo: PrRepository,
}
#[derive(Deserialize)]
struct PrRepository {
    full_name: String,
}

/// Publish only while the PR still points at the tested head; artifacts remain useful if it moved.
pub fn publish(
    api: &Api,
    repository: &str,
    number: u64,
    candidate_sha: &str,
    report: &ImpactReport,
    run_url: &url::Url,
) -> Result<bool, BotError> {
    if !safe_repository(repository)
        || !safe_sha(candidate_sha)
        || run_url.scheme() != "https"
        || !run_url.username().is_empty()
        || run_url.password().is_some()
    {
        return Err(BotError::InvalidPullRequest);
    }
    let pr: PullRequest = api.get(&format!("repos/{repository}/pulls/{number}"), &[])?;
    if pr.state != "open" || pr.head.sha != candidate_sha {
        return Ok(false);
    }
    let body = format!(
        "{}\n\nTested PR head: `{}`. [Full report and build artifacts](<{}>).",
        report.comment_markdown(),
        candidate_sha,
        run_url
    );
    let _: serde_json::Value = api.post(
        &format!("repos/{repository}/issues/{number}/comments"),
        serde_json::json!({"body": body}),
    )?;
    Ok(true)
}
