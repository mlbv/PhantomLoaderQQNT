// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

pub mod cli;
pub mod coordinator;
pub mod image;
pub mod monitor;
pub mod process;
pub mod target;

use cli::Command;
use monitor::Reporter;
use process::SuspendedProcess;
use std::{ffi::OsString, path::PathBuf};
use target::{TargetProfile, ValidatedTarget};

pub type Result<T> = std::result::Result<T, String>;

pub fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<u32> {
    run_command(cli::parse(arguments)?, false)
}

#[cfg(feature = "testhost")]
pub fn run_fixture(arguments: impl IntoIterator<Item = OsString>) -> Result<u32> {
    run_command(cli::parse_fixture(arguments)?, true)
}

fn run_command(command: Command, fixture: bool) -> Result<u32> {
    let options = match command {
        Command::ProfileHash => {
            println!("{}", platform::profile::PROFILE_SHA256);
            return Ok(0);
        }
        Command::Launch(options) => options,
    };
    let reporter = options
        .reporter
        .as_ref()
        .map(|(script, directory)| Reporter::configure(script, directory))
        .transpose()?;
    let executable = match options.executable {
        Some(path) if !path.as_os_str().is_empty() => path,
        _ => {
            let program_files = std::env::var_os("ProgramFiles")
                .filter(|value| !value.is_empty())
                .ok_or("GetEnvironmentVariableW(ProgramFiles) failed")?;
            PathBuf::from(program_files).join("Tencent/QQNT/QQ.exe")
        }
    };
    let mut validated = if fixture {
        None
    } else {
        Some(
            ValidatedTarget::validate(&executable, &TargetProfile::supported())
                .map_err(|error| format!("QQ validation failed: {error}"))?,
        )
    };
    let executable = validated
        .as_ref()
        .map_or(executable, |target| target.executable.clone());
    if options.check_only {
        println!(
            "Supported QQ installation verified: {}",
            executable.display()
        );
        return Ok(0);
    }
    let payload = match options.payload {
        Some(path) => path,
        None => std::env::current_exe()
            .map_err(|error| format!("GetModuleFileNameW(injector) failed: {error}"))?
            .parent()
            .unwrap()
            .join("phantomloader_payload.dll"),
    };
    let prepared = coordinator::PreparedCoordinator::prepare()?;
    let mut process =
        SuspendedProcess::launch_coordinated(&executable, &options.arguments, prepared.name())?;
    let coordinator = prepared.start(&process)?;
    #[cfg(feature = "testhost")]
    match options.test_dll {
        Some(path) => process.inject_test_dll(&path)?,
        None => process.inject_payload(&payload)?,
    }
    #[cfg(not(feature = "testhost"))]
    process.inject_payload(&payload)?;
    if reporter.is_some() {
        process.times()?;
    }
    process.resume()?;
    if let Some(target) = &mut validated {
        target.release_startup_locks();
    }
    println!("PhantomLoaderQQNT initialized in pid {}", process.id);
    let result = if let Some(reporter) = reporter {
        reporter.monitor(&process)
    } else if options.wait {
        match process.wait(30000).and_then(|()| process.exit_code()) {
            Ok(code) => Ok(code),
            Err(_) => {
                eprintln!("target did not exit within 30 seconds");
                process.terminate(1);
                Ok(1)
            }
        }
    } else {
        process.wait_for_exit().map(|_| 0)
    };
    if result.is_ok() {
        coordinator.finish()?;
    }
    result
}
