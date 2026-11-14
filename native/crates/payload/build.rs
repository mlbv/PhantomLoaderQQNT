// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

fn main() {
    for argument in [
        "/NODEFAULTLIB",
        "/ENTRY:PayloadMain",
        "/DYNAMICBASE",
        "/OPT:REF",
        "/INCREMENTAL:NO",
    ] {
        println!("cargo:rustc-link-arg={argument}");
    }
}
