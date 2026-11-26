// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};

use crate::Result;

#[derive(Debug)]
pub struct Options {
    pub executable: Option<PathBuf>,
    pub payload: Option<PathBuf>,
    pub reporter: Option<(PathBuf, PathBuf)>,
    pub wait: bool,
    pub check_only: bool,
    pub arguments: Vec<OsString>,
    #[cfg(feature = "testhost")]
    pub test_dll: Option<PathBuf>,
}

pub enum Command {
    ProfileHash,
    Launch(Options),
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command> {
    parse_mode(arguments, false)
}

#[cfg(feature = "testhost")]
pub fn parse_fixture(arguments: impl IntoIterator<Item = OsString>) -> Result<Command> {
    parse_mode(arguments, true)
}

fn parse_mode(arguments: impl IntoIterator<Item = OsString>, fixture: bool) -> Result<Command> {
    let mut args = arguments.into_iter().fuse().peekable();
    let first = args.next();
    if !fixture && first.as_deref() == Some(OsStr::new("-profile-hash")) && args.peek().is_none() {
        return Ok(Command::ProfileHash);
    }
    let mut args = first.into_iter().chain(args);
    let mut options = Options {
        executable: None,
        payload: None,
        reporter: None,
        wait: false,
        check_only: false,
        arguments: Vec::new(),
        #[cfg(feature = "testhost")]
        test_dll: None,
    };
    let mut reporter = None;
    let mut capture_directory = None;
    while let Some(arg) = args.next() {
        if arg == "--" {
            options.arguments.extend(args);
            break;
        }
        let takes_path = arg == "-exe"
            || arg == "-payload"
            || arg == "-reporter"
            || arg == "-capture-directory"
            || cfg!(feature = "testhost") && fixture && arg == "-dll";
        if takes_path {
            let path =
                PathBuf::from(args.next().ok_or_else(|| {
                    format!("{} requires a path argument", arg.to_string_lossy())
                })?);
            match arg.to_str() {
                Some("-exe") => options.executable = Some(path),
                Some("-payload") => options.payload = Some(path),
                Some("-reporter") => set_once(&mut reporter, path, "-reporter")?,
                Some("-capture-directory") => {
                    set_once(&mut capture_directory, path, "-capture-directory")?
                }
                #[cfg(feature = "testhost")]
                Some("-dll") => options.test_dll = Some(path),
                _ => unreachable!(),
            }
        } else if arg == "-wait" {
            options.wait = true;
        } else if !fixture && arg == "-check" {
            options.check_only = true;
        } else {
            if fixture {
                options.arguments.push(arg);
            } else {
                return Err(format!(
                    "Unknown PhantomLoaderQQNT argument: {}; use -- before QQ arguments",
                    arg.to_string_lossy()
                ));
            }
        }
    }
    options.reporter = match (reporter, capture_directory) {
        (None, None) => None,
        (Some(script), Some(directory))
            if !script.as_os_str().is_empty() && !directory.as_os_str().is_empty() =>
        {
            Some((script, directory))
        }
        _ => return Err("-reporter and -capture-directory require nonempty paths together".into()),
    };
    if options.reporter.is_some() && options.wait {
        return Err("-wait cannot be combined with the capture reporter lifetime monitor".into());
    }
    Ok(Command::Launch(options))
}

fn set_once(value: &mut Option<PathBuf>, path: PathBuf, option: &str) -> Result<()> {
    if value.replace(path).is_some() {
        return Err(format!("{option} may only be specified once"));
    }
    Ok(())
}

pub fn quote_argument(argument: &OsStr) -> OsString {
    let mut quoted = vec![b'"' as u16];
    let mut backslashes = 0;
    for ch in argument.encode_wide() {
        if ch == b'\\' as u16 {
            backslashes += 1;
        } else {
            quoted.extend(std::iter::repeat_n(
                b'\\' as u16,
                if ch == b'"' as u16 {
                    backslashes * 2 + 1
                } else {
                    backslashes
                },
            ));
            quoted.push(ch);
            backslashes = 0;
        }
    }
    quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    quoted.push(b'"' as u16);
    OsString::from_wide(&quoted)
}

pub fn command_line(executable: &OsStr, arguments: &[OsString]) -> OsString {
    let mut command = quote_argument(executable);
    for argument in arguments {
        command.push(" ");
        command.push(quote_argument(argument));
    }
    command
}
