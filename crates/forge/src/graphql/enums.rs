//! Scalars, enums and shared input objects of the GitHub GraphQL schema subset.
use async_graphql::{Enum, InputObject};
use chrono::{DateTime as ChronoDateTime, Utc};
use serde::{Deserialize, Serialize};

/// `DateTime`: ISO-8601 `Z` timestamps without fractional seconds, like GitHub.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct DateTime(pub String);
async_graphql::scalar!(DateTime, "DateTime", "An ISO-8601 encoded UTC date string.");

impl From<ChronoDateTime<Utc>> for DateTime {
    fn from(t: ChronoDateTime<Utc>) -> Self {
        DateTime(crate::json::ts(t))
    }
}

pub fn dt(t: ChronoDateTime<Utc>) -> DateTime {
    t.into()
}

pub fn dt_opt(t: Option<ChronoDateTime<Utc>>) -> Option<DateTime> {
    t.map(Into::into)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct Uri(pub String);
async_graphql::scalar!(Uri, "URI", "An RFC 3986, RFC 3987, and RFC 6570 (level 4) compliant URI string.");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct GitObjectId(pub String);
async_graphql::scalar!(GitObjectId, "GitObjectID", "A Git object ID.");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct GitSshRemote(pub String);
async_graphql::scalar!(GitSshRemote, "GitSSHRemote", "Git SSH string");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct Html(pub String);
async_graphql::scalar!(Html, "HTML", "A string containing HTML code.");

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IssueStateReason {
    Reopened,
    NotPlanned,
    Completed,
    Duplicate,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IssueClosedStateReason {
    Completed,
    NotPlanned,
    Duplicate,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestUpdateState {
    Open,
    Closed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestMergeMethod {
    Merge,
    Squash,
    Rebase,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestReviewDecision {
    ChangesRequested,
    Approved,
    ReviewRequired,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestReviewEvent {
    Comment,
    Approve,
    RequestChanges,
    Dismiss,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestReviewState {
    Pending,
    Commented,
    Approved,
    ChangesRequested,
    Dismissed,
}

impl PullRequestReviewState {
    pub fn parse(s: &str) -> Self {
        match s {
            "APPROVED" => Self::Approved,
            "CHANGES_REQUESTED" => Self::ChangesRequested,
            "DISMISSED" => Self::Dismissed,
            "PENDING" => Self::Pending,
            _ => Self::Commented,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MergeableState {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MergeStateStatus {
    Behind,
    Blocked,
    Clean,
    Dirty,
    Draft,
    HasHooks,
    Unknown,
    Unstable,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RepositoryVisibility {
    Private,
    Public,
    Internal,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RepositoryPermission {
    Admin,
    Maintain,
    Write,
    Triage,
    Read,
}

impl RepositoryPermission {
    pub fn from_perm(p: crate::db::Perm) -> Option<Self> {
        use crate::db::Perm;
        Some(match p {
            Perm::Admin => Self::Admin,
            Perm::Maintain => Self::Maintain,
            Perm::Write => Self::Write,
            Perm::Triage => Self::Triage,
            Perm::Read => Self::Read,
            Perm::None => return None,
        })
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RepositoryPrivacy {
    Public,
    Private,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RepositoryAffiliation {
    Owner,
    Collaborator,
    OrganizationMember,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum OrderDirection {
    Asc,
    Desc,
}

impl OrderDirection {
    pub fn sql(self) -> &'static str {
        match self {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IssueOrderField {
    CreatedAt,
    UpdatedAt,
    Comments,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum LabelOrderField {
    Name,
    CreatedAt,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MilestoneOrderField {
    CreatedAt,
    UpdatedAt,
    DueDate,
    Number,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RepositoryOrderField {
    CreatedAt,
    UpdatedAt,
    PushedAt,
    Name,
    Stargazers,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReleaseOrderField {
    CreatedAt,
    Name,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IssueCommentOrderField {
    UpdatedAt,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RefOrderField {
    TagCommitDate,
    Alphabetical,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectState {
    Open,
    Closed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectOrderField {
    CreatedAt,
    UpdatedAt,
    Name,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectV2OrderField {
    Title,
    Number,
    UpdatedAt,
    CreatedAt,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TeamOrderField {
    Name,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum LanguageOrderField {
    Size,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MilestoneState {
    Open,
    Closed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommentAuthorAssociation {
    Member,
    Owner,
    Mannequin,
    Collaborator,
    Contributor,
    FirstTimeContributor,
    FirstTimer,
    #[graphql(name = "NONE")]
    NoAssociation,
}

impl CommentAuthorAssociation {
    pub fn parse(s: &str) -> Self {
        match s {
            "OWNER" => Self::Owner,
            "MEMBER" => Self::Member,
            "COLLABORATOR" => Self::Collaborator,
            "CONTRIBUTOR" => Self::Contributor,
            "FIRST_TIME_CONTRIBUTOR" => Self::FirstTimeContributor,
            "FIRST_TIMER" => Self::FirstTimer,
            _ => Self::NoAssociation,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReactionContent {
    ThumbsUp,
    ThumbsDown,
    Laugh,
    Hooray,
    Confused,
    Heart,
    Rocket,
    Eyes,
}

impl ReactionContent {
    pub const ALL: [(ReactionContent, &'static str); 8] = [
        (ReactionContent::ThumbsUp, "+1"),
        (ReactionContent::ThumbsDown, "-1"),
        (ReactionContent::Laugh, "laugh"),
        (ReactionContent::Hooray, "hooray"),
        (ReactionContent::Confused, "confused"),
        (ReactionContent::Heart, "heart"),
        (ReactionContent::Rocket, "rocket"),
        (ReactionContent::Eyes, "eyes"),
    ];
}

/// Deliberately without ISSUE_ADVANCED / ISSUE_SEMANTIC / ISSUE_HYBRID (gh then sends `type: ISSUE`).
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SearchType {
    Issue,
    Repository,
    User,
    Discussion,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum StatusState {
    Expected,
    Error,
    Failure,
    Pending,
    Success,
}

impl StatusState {
    pub fn parse(s: &str) -> Self {
        match s {
            "error" => Self::Error,
            "failure" => Self::Failure,
            "pending" => Self::Pending,
            "success" => Self::Success,
            _ => Self::Expected,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CheckStatusState {
    Requested,
    Queued,
    InProgress,
    Completed,
    Waiting,
    Pending,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CheckConclusionState {
    ActionRequired,
    TimedOut,
    Cancelled,
    Failure,
    Success,
    Neutral,
    Skipped,
    StartupFailure,
    Stale,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PatchStatus {
    Added,
    Deleted,
    Renamed,
    Copied,
    Modified,
    Changed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SubscriptionState {
    Unsubscribed,
    Subscribed,
    Ignored,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum LockReason {
    OffTopic,
    TooHeated,
    Resolved,
    Spam,
}

impl LockReason {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off-topic" | "off_topic" => Self::OffTopic,
            "too heated" | "too_heated" => Self::TooHeated,
            "resolved" => Self::Resolved,
            "spam" => Self::Spam,
            _ => return None,
        })
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DiffSide {
    Left,
    Right,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectCardArchivedState {
    Archived,
    NotArchived,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FundingPlatform {
    Github,
    Patreon,
    OpenCollective,
    KoFi,
    Tidelift,
    CommunityBridge,
    Liberapay,
    Issuehunt,
    LfxCrowdfunding,
    Polar,
    BuyMeACoffee,
    ThanksDev,
    Custom,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IssueTypeColor {
    Gray,
    Blue,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
}

macro_rules! order_input {
    ($name:ident, $gql:literal, $field:ty) => {
        #[derive(InputObject, Clone, Copy, Debug)]
        #[graphql(name = $gql)]
        pub struct $name {
            pub field: $field,
            pub direction: OrderDirection,
        }
    };
}

order_input!(IssueOrder, "IssueOrder", IssueOrderField);
order_input!(LabelOrder, "LabelOrder", LabelOrderField);
order_input!(MilestoneOrder, "MilestoneOrder", MilestoneOrderField);
order_input!(RepositoryOrder, "RepositoryOrder", RepositoryOrderField);
order_input!(ReleaseOrder, "ReleaseOrder", ReleaseOrderField);
order_input!(IssueCommentOrder, "IssueCommentOrder", IssueCommentOrderField);
order_input!(RefOrder, "RefOrder", RefOrderField);
order_input!(ProjectOrder, "ProjectOrder", ProjectOrderField);
order_input!(ProjectV2Order, "ProjectV2Order", ProjectV2OrderField);
order_input!(TeamOrder, "TeamOrder", TeamOrderField);
order_input!(LanguageOrder, "LanguageOrder", LanguageOrderField);

#[derive(InputObject, Clone, Debug, Default)]
#[graphql(name = "IssueFilters")]
pub struct IssueFilters {
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub labels: Option<Vec<String>>,
    pub mentioned: Option<String>,
    pub milestone: Option<String>,
    pub milestone_number: Option<String>,
    pub since: Option<DateTime>,
    pub states: Option<Vec<IssueState>>,
    #[graphql(name = "type")]
    pub kind: Option<String>,
    #[graphql(default = false)]
    pub viewer_subscribed: bool,
}
