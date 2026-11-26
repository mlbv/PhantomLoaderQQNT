// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

fn main() {
    let code = phantomloader::run(std::env::args_os().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        1
    });
    std::process::exit(code as i32);
}
