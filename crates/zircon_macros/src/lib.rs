// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The in-process Zircon kernel's attribute for its syscalls (see the `zircon` crate's
//! `syscalls`).

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// Exports a syscall under its C name, as `zx-sys` declares it, returning a `zx_status_t`. Its
/// body is a `Result<(), Status>`, as Zircon's `zx_status_t` returns are: early returns of
/// `Err(status)`, and `?`, fail with the status.
#[proc_macro_attribute]
pub fn syscall(_args: TokenStream, item: TokenStream) -> TokenStream {
    let ItemFn { attrs, vis, sig, block } = parse_macro_input!(item as ItemFn);
    quote! {
        #(#attrs)*
        #[unsafe(no_mangle)]
        #vis #sig {
            crate::syscalls::status((|| -> ::core::result::Result<(), ::zx_status::Status> #block)())
        }
    }
    .into()
}
