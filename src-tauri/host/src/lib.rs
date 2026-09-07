//! Host-owned operations, shared by the desktop and the standalone stdio helper.
pub mod cancellation;
pub mod content;
pub mod dev_server_detect;
#[cfg(unix)]
mod dev_server_stream;
pub mod file_content;
#[cfg(unix)]
pub mod files;
pub mod git_command;
pub mod git_log;
pub mod git_oid;
pub mod git_process;
pub mod git_registry;
pub mod git_service;
pub mod git_status;
pub mod git_watch;
pub mod herdr_backend;
pub mod herdr_command;
pub mod herdr_limits;
pub mod herdr_service;
mod herdr_transport;
#[cfg(unix)]
pub mod login_env;
pub mod path_capability;
pub mod preview_resource_policy;
pub mod preview_server;
pub mod process_kill;
pub mod process_service;
pub mod protocol;
#[cfg(unix)]
pub mod server;
pub mod shell;
pub mod stream_protocol;
#[cfg(unix)]
pub mod streams;
pub mod trust_command;
pub mod tunnel;
pub mod watcher;
pub mod wire;
pub mod workspace_path_index;
pub mod workspace_trust;

pub mod search;
#[cfg(unix)]
mod search_stream;

pub mod log_event;
pub mod lsp_adapters;
pub mod lsp_catalog;
pub mod lsp_command;
pub mod lsp_config;
pub mod lsp_download;
pub mod lsp_framing;
pub mod lsp_install_verification;
pub mod lsp_plan;
pub mod lsp_service;

#[cfg(unix)]
mod lsp_install_stream;
#[cfg(unix)]
mod lsp_stream;

#[cfg(unix)]
pub mod stdio;

pub mod db_connection_actor;
pub mod db_endpoint;
pub mod db_logging;
pub mod db_query_worker;
pub mod db_remote;
pub mod db_result_session;
pub mod db_service;
#[cfg(unix)]
pub mod sqlite_lane;
