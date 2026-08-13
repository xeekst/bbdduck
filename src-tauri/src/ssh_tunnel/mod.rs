//! SSH 端口转发（隧道）模块：本地 / 远程 / 动态（SOCKS5），TCP/UDP。
//! 基于 russh + tokio，支持多个隧道同时运行、随时停止、自动保活与断线重连。

pub mod manager;
pub mod model;
pub mod runner;

pub use manager::TunnelManager;
