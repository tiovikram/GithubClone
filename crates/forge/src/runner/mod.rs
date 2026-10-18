//! GitHub Actions execution (area "runner").
//!
//! Server side (inside `forge serve`): [`trigger`] turns repository events, dispatches and
//! cron schedules into workflow runs; [`engine`] plans jobs (needs, `if:`, matrix) and
//! aggregates their results; [`protocol`] is the HTTP API runners poll; [`logs`] and
//! [`storage`] keep logs, artifacts and caches in object storage; [`token`] issues job tokens
//! (`GITHUB_TOKEN`) and runner credentials; [`settings`] is the one call site for Actions
//! settings (secrets, variables, allowed, default token permission).
//!
//! Runner side (`forge runner`, a separate process/pod — jobs never run in the API server):
//! [`agent`] registers, long-polls for jobs and executes them with [`exec`].
pub mod agent;
pub mod cron;
pub mod engine;
pub mod builtin;
pub mod exec;
pub mod expr;
pub mod glob;
pub mod logs;
pub mod protocol;
pub mod settings;
pub mod storage;
pub mod token;
pub mod trigger;
pub mod workflow;
pub mod zip;
