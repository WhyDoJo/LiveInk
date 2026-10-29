mod app;
mod drawing;
mod ipc;
mod renderer;
mod ui;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut command = "toggle".to_owned();
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = Some(args.next().context("--output needs a monitor name")?),
            "--help" | "-h" => {
                println!(
                    "LivePaint — live Wayland screen annotation\n\nUsage: livepaint [toggle|show|hide|passthrough|clear|undo|redo|quit] [--output NAME]\n\nDefault: toggle. Starts an overlay if not running.\n--output selects a monitor on startup (otherwise the focused Hyprland monitor).\n\nP pen · L line · A arrow · R rectangle · O ellipse · E eraser\n1–6 color · [ / ] width · Ctrl+Z undo · Ctrl+Shift+Z redo\nTab click-through · Escape hide · Ctrl+Q quit · F1 help\n\nSee README.md for installation and global Hyprland shortcuts."
                );
                return Ok(());
            }
            "toggle" | "show" | "hide" | "passthrough" | "clear" | "undo" | "redo" | "quit" => {
                command = arg
            }
            _ => bail!("Unknown argument: {arg}. Try --help"),
        }
    }
    let command = ipc::Command::parse(&command).context("Invalid command")?;
    if ipc::send(command)? {
        return Ok(());
    }
    if !matches!(command, ipc::Command::Toggle | ipc::Command::Show) {
        bail!("LivePaint is not running. Start it with: livepaint");
    }
    app::run(output)
}
