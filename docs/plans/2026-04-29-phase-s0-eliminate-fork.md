# Phase S0: Eliminate tarpc fork — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move all fory-specific code from justinholmes/tarpc fork into tarpc-fory + new tarpc-fory-macros crate. Delete the fork dependency. Depend on upstream tarpc from crates.io.

**Architecture:** Two composable attributes replace the monolithic fork proc-macro. `#[tarpc::service]` (upstream, unmodified) runs first and emits `XxxRequest`/`XxxResponse` enums + client/server machinery. `#[tarpc_fory::fory_service]` runs second on the expanded token stream: it parses the generated enums, emits manual fory `Serializer` impls (EXT path), `ForyDefault` impls, a `XxxService` marker struct, and `ServiceWireSchema` impl. The transport code (codec, TCP connect/listen, TLS, zero-copy) moves verbatim from the fork's `serde_transport/fory*.rs` into `tarpc-fory/src/`. Import paths change from `crate::` (tarpc internals) to `tarpc::` (upstream re-exports).

**Tech Stack:** Rust, tarpc 0.37, fory 0.17, tokio-rustls 0.26, bytes 1, criterion 0.5

**Current LOC in fork:** 633 (fory.rs) + 660 (fory_envelope.rs) + 1076 (fory_zerocopy.rs) + ~460 fory-specific lines in plugins/src/lib.rs = ~2829 LOC. Plus 2169 LOC of tests/benches.

## Critical dependency: `tarpc::serde_transport::{new, Transport}`

The transport code uses `tarpc::serde_transport::new()` and `tarpc::serde_transport::Transport`. These are public in the fork (verified: `pub fn new<S, Item, SinkItem, Codec>` and `pub struct Transport<S, Item, SinkItem, Codec>` in `serde_transport.rs`). Upstream tarpc 0.37 on crates.io exposes the same `serde_transport` module with the `serde-transport` feature. Confirmed: `Transport` and `new` are part of tarpc's core `serde_transport` module, not added by the fork. The fork only adds the `fory`, `fory_envelope`, and `fory_zerocopy` sub-modules.

Other upstream types used: `tarpc::{ClientMessage, Response, Request, ServerError, trace, context, server, client}` — all public in upstream tarpc 0.37.

## Wire ID migration note

`fory_wire_id` uses `concat!(module_path!(), "::", ...)` which is evaluated at compile time in the user's crate. After the move, the `ServiceWireSchema::register` impl emitted by the proc-macro still uses `module_path!()` evaluated in the user's crate — so wire IDs for `XxxRequest`/`XxxResponse` do not change. However, the proc-macro's emitted code will reference `::tarpc_fory::envelope::fory_wire_id` instead of `::tarpc::serde_transport::fory_envelope::fory_wire_id`. The function itself is identical; only the import path changes. Wire IDs remain stable.

---

## Task 1: Create tarpc-fory-macros crate skeleton

**Files:** `~/work/fory-rpc/tarpc-fory-macros/Cargo.toml`, `~/work/fory-rpc/tarpc-fory-macros/src/lib.rs`

- [ ] 1.1 Create `~/work/fory-rpc/tarpc-fory-macros/Cargo.toml`:

```toml
[package]
name = "tarpc-fory-macros"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Proc-macro companion for tarpc-fory: #[fory_service] attribute"
repository = "https://github.com/justinholmes/tarpc-fory"

[lib]
proc-macro = true

[dependencies]
syn = { version = "2", features = ["full", "parsing", "printing", "visit"] }
quote = "1"
proc-macro2 = "1"
```

- [ ] 1.2 Create `~/work/fory-rpc/tarpc-fory-macros/src/lib.rs` with a minimal stub that passes through unchanged:

```rust
extern crate proc_macro;

use proc_macro::TokenStream;

/// Fory wire-schema attribute for tarpc services.
///
/// Apply AFTER `#[tarpc::service]` on a trait definition. Parses the
/// tarpc-generated `XxxRequest` / `XxxResponse` enums from the expanded
/// token stream and emits:
///
/// - Manual `fory::Serializer` + `fory::ForyDefault` impls (EXT path)
/// - `pub struct XxxService;` marker
/// - `impl tarpc_fory::ServiceWireSchema for XxxService`
/// - User-type registration calls via `fory.register()`
///
/// # Example
///
/// ```ignore
/// #[tarpc::service]
/// #[tarpc_fory::fory_service]
/// trait Hello {
///     async fn hello(name: String) -> String;
/// }
/// ```
#[proc_macro_attribute]
pub fn fory_service(_attr: TokenStream, input: TokenStream) -> TokenStream {
    // Phase: skeleton — pass-through. Task 2 fills in the real logic.
    input
}
```

- [ ] 1.3 Verify: `cd ~/work/fory-rpc/tarpc-fory-macros && cargo check`

```bash
cd ~/work/fory-rpc/tarpc-fory-macros && cargo check
```

Expected: compiles with no errors.

- [ ] 1.4 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory-macros
git init
git add -A
git commit -m "feat(macros): tarpc-fory-macros crate skeleton with pass-through #[fory_service]"
```

---

## Task 2: Extract the `#[fory_service]` proc-macro logic

**Source:** `~/work/fory-rpc/tarpc-fork/plugins/src/lib.rs` lines 842-1258 (fory_impls, collect_user_type_registrations, collect_type_registrations, is_builtin_type_name)

**Target:** `~/work/fory-rpc/tarpc-fory-macros/src/lib.rs`

The key difference from the fork: in the fork, `fory_impls()` is a method on `ServiceGenerator` which has access to all parsed fields (ident, rpcs, args, return_types, camel_case_idents, etc.) because `#[tarpc::service]` parses the trait and builds them. In the standalone macro, `#[fory_service]` receives the ALREADY-EXPANDED output of `#[tarpc::service]` as a token stream. It must re-parse the token stream to extract the information it needs.

### What `#[fory_service]` receives

After `#[tarpc::service]` expands, the token stream contains (in order):
1. The trait definition (e.g., `pub trait Hello { ... }`)
2. `pub trait HelloStub: ...`
3. `impl<S> HelloStub for S ...`
4. `pub struct ServeHello<S> { ... }`
5. `impl<S> Serve for ServeHello<S> ...`
6. **`pub enum HelloRequest { Greet { name: String }, ... }`** — this is what we need
7. **`pub enum HelloResponse { Greet(String), ... }`** — this too
8. `pub struct HelloClient<Stub = ...>(Stub);`
9. Client impls

### Parsing strategy

Walk the token stream looking for:
- `enum XxxRequest { ... }` where Xxx is derived from the trait name
- `enum XxxResponse { ... }` where Xxx is derived from the trait name

We find the trait name from the first `trait Xxx` in the stream. Then look for `XxxRequest` and `XxxResponse` enums.

- [ ] 2.1 Replace `~/work/fory-rpc/tarpc-fory-macros/src/lib.rs` with the full implementation:

```rust
extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Fields, Ident, Item, ItemEnum, ReturnType, Type, parse_macro_input,
    punctuated::Punctuated, token::Comma, Field,
};

/// Fory wire-schema attribute for tarpc services.
///
/// Apply AFTER `#[tarpc::service]` on a trait definition. Parses the
/// tarpc-generated `XxxRequest` / `XxxResponse` enums from the expanded
/// token stream and emits:
///
/// - Manual `fory::Serializer` + `fory::ForyDefault` impls (EXT path)
/// - `pub struct XxxService;` marker
/// - `impl tarpc_fory::ServiceWireSchema for XxxService`
/// - User-type registration calls via `fory.register()`
///
/// # Example
///
/// ```ignore
/// #[tarpc::service]
/// #[tarpc_fory::fory_service]
/// trait Hello {
///     async fn hello(name: String) -> String;
/// }
/// ```
#[proc_macro_attribute]
pub fn fory_service(_attr: TokenStream, input: TokenStream) -> TokenStream {
    let input2: TokenStream2 = input.clone().into();

    // Parse the expanded token stream as a sequence of items.
    let items: Vec<Item> = {
        // Wrap in a module to parse multiple items.
        let wrapped: TokenStream = format!("mod __parse_wrapper {{ {} }}", input2).parse().unwrap();
        let module: syn::ItemMod = syn::parse(wrapped).expect("fory_service: failed to parse expanded tarpc output");
        module.content.map(|(_, items)| items).unwrap_or_default()
    };

    // Find the trait name.
    let trait_ident = items.iter().find_map(|item| {
        if let Item::Trait(t) = item {
            // Skip the Stub trait (e.g., HelloStub)
            let name = t.ident.to_string();
            if !name.ends_with("Stub") {
                return Some(t.ident.clone());
            }
        }
        None
    }).expect("fory_service: could not find service trait in expanded output");

    let request_ident = format_ident!("{}Request", trait_ident);
    let response_ident = format_ident!("{}Response", trait_ident);

    // Find the Request enum.
    let request_enum: &ItemEnum = items.iter().find_map(|item| {
        if let Item::Enum(e) = item {
            if e.ident == request_ident {
                return Some(e);
            }
        }
        None
    }).unwrap_or_else(|| panic!("fory_service: could not find enum {} in expanded output", request_ident));

    // Find the Response enum.
    let response_enum: &ItemEnum = items.iter().find_map(|item| {
        if let Item::Enum(e) = item {
            if e.ident == response_ident {
                return Some(e);
            }
        }
        None
    }).unwrap_or_else(|| panic!("fory_service: could not find enum {} in expanded output", response_ident));

    // Generate the fory impls.
    let fory_tokens = generate_fory_impls(
        &trait_ident,
        &request_ident,
        &response_ident,
        request_enum,
        response_enum,
    );

    // Emit the original token stream + the fory additions.
    let mut output: TokenStream2 = input.into();
    output.extend(fory_tokens);
    output.into()
}

/// Generate all fory-related impls for the request/response enums.
fn generate_fory_impls(
    service_ident: &Ident,
    request_ident: &Ident,
    response_ident: &Ident,
    request_enum: &ItemEnum,
    response_enum: &ItemEnum,
) -> TokenStream2 {
    // -----------------------------------------------------------------------
    // XxxRequest: build per-variant write/read arms
    // -----------------------------------------------------------------------

    let mut req_write_arms = Vec::new();
    let mut req_read_arms = Vec::new();

    for (idx, variant) in request_enum.variants.iter().enumerate() {
        let disc = idx as u32;
        let variant_ident = &variant.ident;

        match &variant.fields {
            Fields::Named(fields) => {
                let field_idents: Vec<&Ident> = fields.named.iter()
                    .map(|f| f.ident.as_ref().unwrap())
                    .collect();
                let field_types: Vec<&Type> = fields.named.iter()
                    .map(|f| &f.ty)
                    .collect();

                let field_writes = field_idents.iter().map(|fi| quote! {
                    #fi.fory_write(context, ::fory_core::types::RefMode::None, false, false)?;
                }).collect::<Vec<_>>();

                let field_reads = field_idents.iter().zip(field_types.iter()).map(|(fi, ft)| {
                    quote! {
                        let #fi = <#ft>::fory_read(context, ::fory_core::types::RefMode::None, false)?;
                    }
                }).collect::<Vec<_>>();

                req_write_arms.push(quote! {
                    #request_ident::#variant_ident { #( #field_idents ),* } => {
                        (#disc as u32).fory_write(context, ::fory_core::types::RefMode::None, false, false)?;
                        #( #field_writes )*
                    }
                });

                req_read_arms.push(quote! {
                    #disc => {
                        #( #field_reads )*
                        Ok(#request_ident::#variant_ident { #( #field_idents ),* })
                    }
                });
            }
            Fields::Unit => {
                req_write_arms.push(quote! {
                    #request_ident::#variant_ident => {
                        (#disc as u32).fory_write(context, ::fory_core::types::RefMode::None, false, false)?;
                    }
                });
                req_read_arms.push(quote! {
                    #disc => {
                        Ok(#request_ident::#variant_ident)
                    }
                });
            }
            Fields::Unnamed(_) => {
                panic!("fory_service: request enum variants must be named or unit, got unnamed tuple variant {}", variant_ident);
            }
        }
    }

    // ForyDefault for XxxRequest: first variant with all fields defaulted.
    let req_default = {
        let first = &request_enum.variants[0];
        let first_ident = &first.ident;
        match &first.fields {
            Fields::Named(fields) => {
                let defaults = fields.named.iter().map(|f| {
                    let fi = f.ident.as_ref().unwrap();
                    let ft = &f.ty;
                    quote! { #fi: <#ft as ::fory::ForyDefault>::fory_default() }
                }).collect::<Vec<_>>();
                quote! { #request_ident::#first_ident { #( #defaults ),* } }
            }
            Fields::Unit => quote! { #request_ident::#first_ident },
            _ => unreachable!(),
        }
    };

    // -----------------------------------------------------------------------
    // XxxResponse: build per-variant write/read arms
    //
    // tarpc generates: enum XxxResponse { MethodA(RetType), MethodB(RetType) }
    // (unnamed single-field tuple variants)
    // -----------------------------------------------------------------------

    let mut resp_write_arms = Vec::new();
    let mut resp_read_arms = Vec::new();

    for (idx, variant) in response_enum.variants.iter().enumerate() {
        let disc = idx as u32;
        let variant_ident = &variant.ident;

        match &variant.fields {
            Fields::Unnamed(fields) => {
                assert_eq!(fields.unnamed.len(), 1,
                    "fory_service: response variant {} has {} fields, expected 1",
                    variant_ident, fields.unnamed.len());
                let ret_ty = &fields.unnamed[0].ty;

                resp_write_arms.push(quote! {
                    #response_ident::#variant_ident(__v) => {
                        (#disc as u32).fory_write(context, ::fory_core::types::RefMode::None, false, false)?;
                        __v.fory_write(context, ::fory_core::types::RefMode::None, false, false)?;
                    }
                });

                resp_read_arms.push(quote! {
                    #disc => {
                        let __v = <#ret_ty>::fory_read(context, ::fory_core::types::RefMode::None, false)?;
                        Ok(#response_ident::#variant_ident(__v))
                    }
                });
            }
            _ => {
                panic!("fory_service: response enum variants must be unnamed tuple, got {:?} for {}", variant.fields, variant_ident);
            }
        }
    }

    // ForyDefault for XxxResponse: first variant with defaulted inner value.
    let resp_default = {
        let first = &response_enum.variants[0];
        let first_ident = &first.ident;
        let first_ty = match &first.fields {
            Fields::Unnamed(f) => &f.unnamed[0].ty,
            _ => unreachable!(),
        };
        quote! { #response_ident::#first_ident(<#first_ty as ::fory::ForyDefault>::fory_default()) }
    };

    // -----------------------------------------------------------------------
    // ServiceWireSchema impl
    // -----------------------------------------------------------------------

    let req_name = format!("{}Request", service_ident);
    let resp_name = format!("{}Response", service_ident);
    let req_id_expr = quote! {
        ::tarpc_fory::fory_wire_id(
            concat!(module_path!(), "::", #req_name)
        )
    };
    let resp_id_expr = quote! {
        ::tarpc_fory::fory_wire_id(
            concat!(module_path!(), "::", #resp_name)
        )
    };

    // Collect user-type registration calls.
    let user_type_regs = collect_user_type_registrations_from_enums(request_enum, response_enum);

    let service_marker_ident = format_ident!("{}Service", service_ident);

    quote! {
        impl ::fory::ForyDefault for #request_ident {
            fn fory_default() -> Self {
                #req_default
            }
        }

        impl ::fory::Serializer for #request_ident {
            fn fory_write_data(
                &self,
                context: &mut ::fory::WriteContext,
            ) -> ::core::result::Result<(), ::fory::Error> {
                match self {
                    #( #req_write_arms )*
                }
                Ok(())
            }

            fn fory_read_data(
                context: &mut ::fory::ReadContext,
            ) -> ::core::result::Result<Self, ::fory::Error>
            where
                Self: Sized + ::fory::ForyDefault,
            {
                let __disc = u32::fory_read(context, ::fory_core::types::RefMode::None, false)?;
                match __disc {
                    #( #req_read_arms )*
                    _ => Err(::fory::Error::invalid_data(format!(
                        "{}: unknown variant {}",
                        stringify!(#request_ident),
                        __disc,
                    ))),
                }
            }

            fn fory_type_id_dyn(
                &self,
                type_resolver: &::fory::TypeResolver,
            ) -> ::core::result::Result<::fory::TypeId, ::fory::Error> {
                Self::fory_get_type_id(type_resolver)
            }

            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }
        }

        impl ::fory::ForyDefault for #response_ident {
            fn fory_default() -> Self {
                #resp_default
            }
        }

        impl ::fory::Serializer for #response_ident {
            fn fory_write_data(
                &self,
                context: &mut ::fory::WriteContext,
            ) -> ::core::result::Result<(), ::fory::Error> {
                match self {
                    #( #resp_write_arms )*
                }
                Ok(())
            }

            fn fory_read_data(
                context: &mut ::fory::ReadContext,
            ) -> ::core::result::Result<Self, ::fory::Error>
            where
                Self: Sized + ::fory::ForyDefault,
            {
                let __disc = u32::fory_read(context, ::fory_core::types::RefMode::None, false)?;
                match __disc {
                    #( #resp_read_arms )*
                    _ => Err(::fory::Error::invalid_data(format!(
                        "{}: unknown variant {}",
                        stringify!(#response_ident),
                        __disc,
                    ))),
                }
            }

            fn fory_type_id_dyn(
                &self,
                type_resolver: &::fory::TypeResolver,
            ) -> ::core::result::Result<::fory::TypeId, ::fory::Error> {
                Self::fory_get_type_id(type_resolver)
            }

            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }
        }

        /// Marker struct for the service.
        ///
        /// Pass this as the type parameter to [`tarpc_fory::connect`] and
        /// [`tarpc_fory::listen`] for zero-boilerplate fory transport.
        pub struct #service_marker_ident;

        impl ::tarpc_fory::ServiceWireSchema for #service_marker_ident {
            type Req = #request_ident;
            type Resp = #response_ident;

            fn register(fory: &mut ::fory::Fory) -> ::core::result::Result<(), ::fory::Error> {
                use ::tarpc_fory::envelope::{
                    ForyTraceContext, ForyServerError,
                    ForyRequest, ForyResponse, ForyClientMessage,
                };
                // Non-generic envelope types (shared across all requests/responses).
                fory.register_serializer::<ForyTraceContext>(2)?;
                fory.register_serializer::<ForyServerError>(3)?;
                // ID 4 intentionally unassigned (was ForyResult<T>, now removed).
                // Request-side parameterised envelope types.
                fory.register_serializer::<ForyRequest<#request_ident>>(5)?;
                fory.register_serializer::<ForyClientMessage<#request_ident>>(7)?;
                // Response-side parameterised envelope type.
                fory.register_serializer::<ForyResponse<#response_ident>>(6)?;
                // Generated request/response enums (EXT path).
                fory.register_serializer::<#request_ident>(#req_id_expr)?;
                fory.register_serializer::<#response_ident>(#resp_id_expr)?;
                // User-defined types referenced in method signatures.
                #( #user_type_regs )*
                Ok(())
            }
        }
    }
}

/// Collect fory registration calls for user-defined types found in enum fields.
///
/// Walks the request enum's named fields and the response enum's tuple fields.
/// Skips built-in types; emits `fory.register::<T>(fory_wire_id(type_name::<T>()))?;`
/// for each user-defined type.
fn collect_user_type_registrations_from_enums(
    request_enum: &ItemEnum,
    response_enum: &ItemEnum,
) -> Vec<TokenStream2> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut registrations = Vec::new();

    // Request enum: named fields.
    for variant in &request_enum.variants {
        if let Fields::Named(fields) = &variant.fields {
            for field in &fields.named {
                collect_type_registrations(&field.ty, &mut seen, &mut registrations);
            }
        }
    }

    // Response enum: unnamed tuple fields (return types).
    for variant in &response_enum.variants {
        if let Fields::Unnamed(fields) = &variant.fields {
            for field in &fields.unnamed {
                collect_type_registrations(&field.ty, &mut seen, &mut registrations);
            }
        }
    }

    registrations
}

/// Walk a `syn::Type`, skip builtins/primitives, and for each non-trivial path type
/// emit a `fory.register::<T>(fory_wire_id(type_name::<T>()))?;` call.
fn collect_type_registrations(
    ty: &Type,
    seen: &mut std::collections::BTreeSet<String>,
    out: &mut Vec<TokenStream2>,
) {
    match ty {
        Type::Path(type_path) => {
            let last_seg = type_path.path.segments.last();
            if let Some(seg) = last_seg {
                let name = seg.ident.to_string();
                if is_builtin_type_name(&name) {
                    // Recurse into generic args (e.g., Vec<UserData>).
                    if let syn::PathArguments::AngleBracketed(ref args) = seg.arguments {
                        for arg in &args.args {
                            if let syn::GenericArgument::Type(inner_ty) = arg {
                                collect_type_registrations(inner_ty, seen, out);
                            }
                        }
                    }
                } else {
                    let path = &type_path.path;
                    let key = quote! { #path }.to_string();
                    if seen.insert(key) {
                        out.push(quote! {
                            fory.register::<#path>(
                                ::tarpc_fory::fory_wire_id(
                                    ::std::any::type_name::<#path>()
                                )
                            )?;
                        });
                    }
                }
            }
        }
        Type::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_type_registrations(elem, seen, out);
            }
        }
        Type::Reference(r) => collect_type_registrations(&r.elem, seen, out),
        Type::Paren(p) => collect_type_registrations(&p.elem, seen, out),
        Type::Group(g) => collect_type_registrations(&g.elem, seen, out),
        _ => {}
    }
}

/// Returns true for type names that fory handles natively (primitives, std containers).
fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
        | "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
        | "f32" | "f64"
        | "bool" | "char"
        | "String" | "str"
        | "Vec" | "VecDeque" | "LinkedList"
        | "Option" | "Result"
        | "HashMap" | "BTreeMap" | "IndexMap"
        | "HashSet" | "BTreeSet" | "IndexSet"
        | "Box" | "Arc" | "Rc"
        | "Cow"
        | "Duration" | "Instant" | "SystemTime"
        | "PathBuf" | "Path"
        | "Bytes" | "BytesMut"
    )
}
```

- [ ] 2.2 Verify: `cd ~/work/fory-rpc/tarpc-fory-macros && cargo check`

```bash
cd ~/work/fory-rpc/tarpc-fory-macros && cargo check
```

Expected: compiles with no errors.

- [ ] 2.3 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory-macros
git add -A
git commit -m "feat(macros): extract #[fory_service] proc-macro from tarpc fork"
```

---

## Task 3: Move fory_envelope.rs to tarpc-fory/src/envelope.rs

**Source:** `~/work/fory-rpc/tarpc-fork/tarpc/src/serde_transport/fory_envelope.rs` (660 LOC)
**Target:** `~/work/fory-rpc/tarpc-fory/src/envelope.rs`

- [ ] 3.1 Copy the file:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/src/serde_transport/fory_envelope.rs \
   ~/work/fory-rpc/tarpc-fory/src/envelope.rs
```

- [ ] 3.2 Update imports in `~/work/fory-rpc/tarpc-fory/src/envelope.rs`. Replace:

```rust
use crate::{ClientMessage, Request, Response, ServerError, context, trace};
```

with:

```rust
use tarpc::{ClientMessage, Request, Response, ServerError, context, trace};
```

- [ ] 3.3 Update the doc comments that reference `crate::serde_transport::fory_envelope` paths — these should reference `tarpc_fory::envelope` instead. Specifically:

In the module-level doc, replace:
```rust
//! [`crate::serde_transport::fory_envelope`]
```
with:
```rust
//! [`tarpc_fory::envelope`]
```

Replace any `crate::serde_transport::fory::connect` references with `tarpc_fory::connect`.

- [ ] 3.4 The `ServiceWireSchema` trait doc comment references `tarpc::serde_transport::fory::connect` and `tarpc::serde_transport::fory::listen` — update to `tarpc_fory::connect` and `tarpc_fory::listen`.

- [ ] 3.5 The `From<trace::SamplingDecision> for u8` impl conflicts with orphan rules (both the trait and the type are foreign). In the fork this worked because `trace` was in the same crate. Fix: replace the `From` impl with a helper function:

```rust
fn sampling_to_u8(d: trace::SamplingDecision) -> u8 {
    match d {
        trace::SamplingDecision::Sampled => 1,
        trace::SamplingDecision::Unsampled => 0,
    }
}
```

Update all call sites that use `.into()` or `u8::from()` on `SamplingDecision` to call `sampling_to_u8()` instead. The only call site is in `From<&trace::Context> for ForyTraceContext`:

```rust
impl From<&trace::Context> for ForyTraceContext {
    fn from(tc: &trace::Context) -> Self {
        ForyTraceContext {
            trace_id: u128::from(tc.trace_id),
            span_id: u64::from(tc.span_id),
            sampling: sampling_to_u8(tc.sampling_decision),
        }
    }
}
```

- [ ] 3.6 Verify compile:

```bash
cd ~/work/fory-rpc/tarpc-fory && cargo check
```

Expected: fails because lib.rs still uses old re-exports. That is fine — Task 6 updates lib.rs. For now verify just that `src/envelope.rs` has valid syntax by checking the module in isolation:

```bash
cd ~/work/fory-rpc/tarpc-fory && rustfmt src/envelope.rs --check
```

- [ ] 3.7 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add src/envelope.rs
git commit -m "refactor: move fory_envelope.rs from tarpc fork to tarpc-fory/src/envelope.rs"
```

---

## Task 4: Move fory.rs to tarpc-fory/src/transport.rs

**Source:** `~/work/fory-rpc/tarpc-fork/tarpc/src/serde_transport/fory.rs` (633 LOC)
**Target:** `~/work/fory-rpc/tarpc-fory/src/transport.rs`

- [ ] 4.1 Copy the file:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/src/serde_transport/fory.rs \
   ~/work/fory-rpc/tarpc-fory/src/transport.rs
```

- [ ] 4.2 Update imports in `~/work/fory-rpc/tarpc-fory/src/transport.rs`. Replace:

```rust
use super::fory_envelope::{ForyClientMessage, ForyResponse};
use crate::{ClientMessage, Response};
```

with:

```rust
use crate::envelope::{ForyClientMessage, ForyResponse};
use tarpc::{ClientMessage, Response};
```

- [ ] 4.3 Replace:

```rust
use crate::serde_transport::{self, Transport};
```

with:

```rust
use tarpc::serde_transport::{self, Transport};
```

- [ ] 4.4 In the `tcp` module, replace:

```rust
use super::super::fory_envelope::ServiceWireSchema;
```

with:

```rust
use crate::envelope::ServiceWireSchema;
```

- [ ] 4.5 In the `tls` module, replace:

```rust
use super::super::fory_envelope::ServiceWireSchema;
```

with:

```rust
use crate::envelope::ServiceWireSchema;
```

- [ ] 4.6 Update doc comment paths from `crate::serde_transport::fory` to `tarpc_fory::transport`.

- [ ] 4.7 Update the feature gate: the fork uses `#[cfg(feature = "tcp")]` and `#[cfg(all(feature = "tcp", feature = "serde-transport-fory-tls"))]`. In tarpc-fory we control our own features. Define features in Cargo.toml (Task 7) and use:
  - `#[cfg(feature = "tcp")]` remains the same
  - `#[cfg(all(feature = "tcp", feature = "tls"))]` for TLS (simplified feature name)

- [ ] 4.8 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add src/transport.rs
git commit -m "refactor: move fory.rs from tarpc fork to tarpc-fory/src/transport.rs"
```

---

## Task 5: Move fory_zerocopy.rs to tarpc-fory/src/zerocopy.rs

**Source:** `~/work/fory-rpc/tarpc-fork/tarpc/src/serde_transport/fory_zerocopy.rs` (1076 LOC)
**Target:** `~/work/fory-rpc/tarpc-fory/src/zerocopy.rs`

- [ ] 5.1 Copy the file:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/src/serde_transport/fory_zerocopy.rs \
   ~/work/fory-rpc/tarpc-fory/src/zerocopy.rs
```

- [ ] 5.2 Update imports in `~/work/fory-rpc/tarpc-fory/src/zerocopy.rs`. Replace:

```rust
use super::fory_envelope::{ForyClientMessage, ForyResponse};
use crate::{ClientMessage, Response};
```

with:

```rust
use crate::envelope::{ForyClientMessage, ForyResponse};
use tarpc::{ClientMessage, Response};
```

- [ ] 5.3 Update the TLS module's imports similarly. Replace references to `super::super::fory_envelope` with `crate::envelope`.

- [ ] 5.4 Update the feature gate for TLS:

```rust
#[cfg(feature = "tls")]
```

(was `#[cfg(feature = "serde-transport-fory-tls")]` in the fork)

- [ ] 5.5 Update doc comment paths from `tarpc::serde_transport::fory_zerocopy` to `tarpc_fory::zerocopy` and from `crate::serde_transport::fory_envelope` to `tarpc_fory::envelope`.

- [ ] 5.6 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add src/zerocopy.rs
git commit -m "refactor: move fory_zerocopy.rs from tarpc fork to tarpc-fory/src/zerocopy.rs"
```

---

## Task 6: Update tarpc-fory/src/lib.rs

**File:** `~/work/fory-rpc/tarpc-fory/src/lib.rs`

- [ ] 6.1 Replace the entire contents of `~/work/fory-rpc/tarpc-fory/src/lib.rs`:

```rust
//! Apache Fory binary transport for [`tarpc`](https://docs.rs/tarpc).
//!
//! Provides fory-serialized transport, envelope types, and a zero-copy codec
//! for tarpc RPC services. Use `#[tarpc::service]` + `#[tarpc_fory::fory_service]`
//! for zero-boilerplate type registration.
//!
//! # Quick start
//!
//! ```ignore
//! #[tarpc::service]
//! #[tarpc_fory::fory_service]
//! trait Hello {
//!     async fn hello(name: String) -> String;
//! }
//!
//! // Server
//! let mut listener = tarpc_fory::transport::listen::<HelloService, _>("127.0.0.1:0")
//!     .await?;
//!
//! // Client
//! let transport = tarpc_fory::transport::connect::<HelloService, _>(addr).await?;
//! let client = HelloClient::new(client::Config::default(), transport).spawn();
//! ```

pub mod envelope;
pub mod transport;
pub mod zerocopy;

// Re-export the proc-macro.
pub use tarpc_fory_macros::fory_service;

// Re-export key types at crate root for ergonomics.
pub use envelope::{
    fory_wire_id, register_envelope_types, ForyClientMessage, ForyRequest, ForyResponse,
    ForyServerError, ForyTraceContext, ServiceWireSchema,
};
pub use transport::ForyEnvelopeCodec;

// Re-export tokio-serde-fory codec.
pub use tokio_serde_fory::ForyCodec;

// Feature-gated re-exports.
#[cfg(feature = "tcp")]
pub use transport::{connect, connect_with_fory, listen, listen_with_fory, Incoming};

#[cfg(all(feature = "tcp", feature = "tls"))]
pub use transport::{connect_tls, connect_tls_with_fory, listen_tls, listen_tls_with_fory, TlsIncoming};

pub use zerocopy::{
    ClientZeroCopyCodec, ServerZeroCopyCodec, ToFrame, ZeroCopyIncoming,
    ZeroCopyServerTransport, ZeroCopySink, ZeroCopyTransport,
    connect_zerocopy, listen_zerocopy,
};

#[cfg(feature = "tls")]
pub use zerocopy::{
    ZeroCopyTlsIncoming, ZeroCopyTlsTransport, ZeroCopyTlsServerTransport,
    connect_zerocopy_tls, listen_zerocopy_tls,
};
```

- [ ] 6.2 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add src/lib.rs
git commit -m "refactor: replace re-exports with local modules in lib.rs"
```

---

## Task 7: Update tarpc-fory/Cargo.toml

**File:** `~/work/fory-rpc/tarpc-fory/Cargo.toml`

- [ ] 7.1 Replace the entire Cargo.toml:

```toml
[package]
name = "tarpc-fory"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Apache Fory transport for tarpc"
repository = "https://github.com/justinholmes/tarpc-fory"
readme = "README.md"
keywords = ["tarpc", "fory", "rpc", "serialization"]
categories = ["network-programming"]

[features]
default = ["tcp"]
tcp = ["tarpc/tcp", "tokio/net"]
tls = ["tcp", "dep:tokio-rustls"]

[dependencies]
# Upstream tarpc — no fork!
tarpc = { version = "0.37", features = ["serde-transport", "serde1"] }
tarpc-fory-macros = { path = "../tarpc-fory-macros" }
tokio-serde-fory = { path = "../tokio-serde-fory" }

# Fory serialization
fory = "0.17"
fory-core = "0.17"

# Async runtime + IO
tokio = { version = "1", features = ["net", "sync", "time"] }
tokio-util = { version = "0.7", features = ["codec"] }
tokio-serde = "0.9"

# TLS (optional)
tokio-rustls = { version = "0.26", optional = true }

# Framing + streams
futures = "0.3"
pin-project = "1"
bytes = "1"

# serde (needed for Transport bounds)
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
tokio = { version = "1", features = ["full"] }
futures = "0.3"
criterion = { version = "0.5", features = ["html_reports"] }
rcgen = "0.13"

[[bench]]
name = "fory_codec_compare"
harness = false
```

- [ ] 7.2 Key changes from the old Cargo.toml:
  - `tarpc` is now `version = "0.37"` from crates.io, with `features = ["serde-transport", "serde1"]` — NOT `serde-transport-fory` (that feature only exists in the fork)
  - `tarpc-fory-macros` added as path dep
  - `tokio-serde` added (was transitive through tarpc before, now needed directly for `Serializer`/`Deserializer` traits)
  - `pin-project` added (was transitive, now needed for `Incoming`, `ZeroCopyTransport`)
  - `futures` moved from dev-dependencies to dependencies (needed for `Stream` impls)
  - `bytes` added (needed for zerocopy types)
  - `serde` added (needed for Transport's `Serialize`/`Deserialize` bounds)
  - `tokio-rustls` as optional dep behind `tls` feature
  - `rcgen` added to dev-deps for TLS test self-signed certs

- [ ] 7.3 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add Cargo.toml
git commit -m "refactor: depend on upstream tarpc 0.37 from crates.io, drop fork"
```

---

## Task 8: Remove fork proc-macro fory code (decouple)

**File:** `~/work/fory-rpc/tarpc-fork/plugins/src/lib.rs`

The fork's `#[tarpc::service]` proc-macro still emits fory impls via `fory_impls()` (lines 842-1130) guarded by `#[cfg(feature = "fory")]`. After the move, these are dead code — the `fory` feature will no longer be enabled when using upstream tarpc. However, we do NOT modify the fork at this time because:

1. The fork may still be referenced by other consumers
2. We want to verify tarpc-fory works independently first
3. Cleanup is Task 10

- [ ] 8.1 No action needed. Document that the fork's `fory_impls()` is dead code once `tarpc-fory` depends on upstream tarpc.

---

## Task 9: Move and update tests

**Source:** 7 test files from `~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_*.rs` + 1 bench file
**Target:** `~/work/fory-rpc/tarpc-fory/tests/` and `~/work/fory-rpc/tarpc-fory/benches/`

The existing 10 tests in `~/work/fory-rpc/tarpc-fory/tests/` already exercise the tarpc-fory re-export API. After the refactor, these imports change. Additionally, the fork's 7 fory test files test the internal implementation and should be migrated.

### 9.1: Update existing tarpc-fory tests

- [ ] 9.1.1 The 10 existing test files in `~/work/fory-rpc/tarpc-fory/tests/` currently use `tarpc_fory::*` re-exports. After the refactor, the same re-exports are available from the same crate root — just pointing at local modules instead of fork paths. **No import changes needed** in existing tests because they already use `tarpc_fory::*`.

- [ ] 9.1.2 Verify: `cd ~/work/fory-rpc/tarpc-fory && cargo test`

### 9.2: Copy fork test files

- [ ] 9.2.1 Copy envelope tests:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_envelope.rs \
   ~/work/fory-rpc/tarpc-fory/tests/envelope_internals.rs
```

Update imports:
```rust
// Old:
use tarpc::serde_transport::fory_envelope::{...};
// New:
use tarpc_fory::envelope::{...};
// Also update tarpc::serde_transport::fory_envelope to tarpc_fory::envelope
// And tarpc::ServerError to tarpc::ServerError (unchanged — still from upstream)
```

Remove the `#![cfg(feature = "serde-transport-fory")]` gate (always enabled in tarpc-fory).

- [ ] 9.2.2 Copy coverage tests:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_coverage.rs \
   ~/work/fory-rpc/tarpc-fory/tests/coverage.rs
```

Update imports:
```rust
// Old:
use tarpc::serde_transport::fory as fory_transport;
use tarpc::serde_transport::fory_envelope::register_envelope_types;
// New:
use tarpc_fory::transport as fory_transport;
use tarpc_fory::register_envelope_types;
```

Remove `#![cfg(...)]` gate. Replace `#[tarpc::service(...)]` with `#[tarpc::service(...)] #[tarpc_fory::fory_service]` on the `Alpha` and `Beta` traits. Remove `fory_transport::listen::<AlphaService, _>` and replace with the tarpc-fory equivalents.

Note: the `connect::<AlphaService, _>` and `listen::<AlphaService, _>` schema-driven APIs require `ServiceWireSchema` which is now emitted by `#[fory_service]` instead of the fork proc-macro. The `AlphaService` and `BetaService` marker structs are still generated — now by `#[fory_service]` instead of the fork's `fory_impls()`.

- [ ] 9.2.3 Copy transport tests:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_transport.rs \
   ~/work/fory-rpc/tarpc-fory/tests/transport_fork.rs
```

Update imports similarly. The `Hello` service definition needs `#[tarpc_fory::fory_service]` added. The `#[tarpc_plugins::service]` usage changes to `#[tarpc::service]`.

- [ ] 9.2.4 Copy service test:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_service.rs \
   ~/work/fory-rpc/tarpc-fory/tests/service_schema.rs
```

Update:
```rust
// Old:
use tarpc::serde_transport::fory as fory_transport;
#[tarpc::service(derive = [Clone, serde::Serialize, serde::Deserialize])]
trait Hello { ... }
// New:
use tarpc_fory::transport as fory_transport;
#[tarpc::service(derive = [Clone, serde::Serialize, serde::Deserialize])]
#[tarpc_fory::fory_service]
trait Hello { ... }
```

Replace `fory_transport::listen::<HelloService, _>` with `tarpc_fory::listen::<HelloService, _>` (or `tarpc_fory::transport::listen::<HelloService, _>`).
Replace `fory_transport::connect::<HelloService, _>` similarly.

- [ ] 9.2.5 Copy zerocopy tests:

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_zerocopy.rs \
   ~/work/fory-rpc/tarpc-fory/tests/zerocopy_internals.rs
```

Update imports:
```rust
// Old:
use tarpc::serde_transport::fory_envelope::register_envelope_types;
use tarpc::serde_transport::fory_zerocopy::{...};
// New:
use tarpc_fory::register_envelope_types;
use tarpc_fory::zerocopy::{...};
// tarpc::{ClientMessage, Request, Response, context} stays the same (from upstream)
```

- [ ] 9.2.6 Copy TLS tests (only if `tls` feature is implemented):

```bash
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_tls.rs \
   ~/work/fory-rpc/tarpc-fory/tests/tls.rs
cp ~/work/fory-rpc/tarpc-fork/tarpc/tests/fory_zerocopy_tls.rs \
   ~/work/fory-rpc/tarpc-fory/tests/zerocopy_tls.rs
```

Update imports from `tarpc::serde_transport::fory` to `tarpc_fory::transport` and `tarpc_fory::zerocopy`. Add `#[tarpc_fory::fory_service]` to service traits. Replace feature gates with `#![cfg(feature = "tls")]`.

### 9.3: Copy benchmark

- [ ] 9.3.1 Copy the benchmark:

```bash
mkdir -p ~/work/fory-rpc/tarpc-fory/benches
cp ~/work/fory-rpc/tarpc-fork/tarpc/benches/fory_codec_compare.rs \
   ~/work/fory-rpc/tarpc-fory/benches/fory_codec_compare.rs
```

Update imports:
```rust
// Old:
use tarpc::serde_transport::fory_envelope::register_envelope_types;
use tarpc::serde_transport::fory_zerocopy::{...};
use tarpc::serde_transport::fory as fory_transport;
use tarpc::serde_transport::{Transport, new};
// New:
use tarpc_fory::register_envelope_types;
use tarpc_fory::zerocopy::{...};
use tarpc_fory::transport as fory_transport;
use tarpc::serde_transport::{Transport, new};
```

The type aliases using `tarpc::serde_transport::Transport` and `tarpc::serde_transport::fory::ForyEnvelopeCodec` change to use `tarpc::serde_transport::Transport` (unchanged — from upstream) and `tarpc_fory::ForyEnvelopeCodec`.

- [ ] 9.3.2 Ensure `[[bench]]` entry exists in Cargo.toml (added in Task 7).

### 9.4: Declare test files in Cargo.toml

- [ ] 9.4.1 Add test entries to `~/work/fory-rpc/tarpc-fory/Cargo.toml`:

```toml
[[test]]
name = "envelope_internals"
path = "tests/envelope_internals.rs"

[[test]]
name = "coverage"
path = "tests/coverage.rs"

[[test]]
name = "transport_fork"
path = "tests/transport_fork.rs"

[[test]]
name = "service_schema"
path = "tests/service_schema.rs"

[[test]]
name = "zerocopy_internals"
path = "tests/zerocopy_internals.rs"

[[test]]
name = "tls"
path = "tests/tls.rs"
required-features = ["tls"]

[[test]]
name = "zerocopy_tls"
path = "tests/zerocopy_tls.rs"
required-features = ["tls"]
```

- [ ] 9.4.2 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add tests/ benches/ Cargo.toml
git commit -m "test: migrate fork fory tests and benchmark to tarpc-fory"
```

---

## Task 10: Full build + test verification

- [ ] 10.1 Clean build tarpc-fory with upstream tarpc (no path dep on fork):

```bash
cd ~/work/fory-rpc/tarpc-fory && cargo build 2>&1
```

Expected: compiles with no errors. If `tarpc::serde_transport::new` or `tarpc::serde_transport::Transport` is not found, verify that the upstream `serde-transport` feature is enabled in Cargo.toml.

- [ ] 10.2 Run all tests:

```bash
cd ~/work/fory-rpc/tarpc-fory && cargo test --all-features 2>&1
```

Expected: all tests pass. This includes:
- 10 existing tarpc-fory tests (transport, cancel, errors, custom_types, etc.)
- 7 migrated fork tests (envelope_internals, coverage, transport_fork, service_schema, zerocopy_internals, tls, zerocopy_tls)

- [ ] 10.3 Run benchmarks (smoke test, not full benchmark run):

```bash
cd ~/work/fory-rpc/tarpc-fory && cargo bench --bench fory_codec_compare -- --test 2>&1
```

Expected: benchmarks compile and run in test mode without errors.

- [ ] 10.4 Canonical proof test. Create a file `~/work/fory-rpc/tarpc-fory/tests/canonical_no_fork.rs`:

```rust
//! Canonical proof: #[tarpc::service] + #[tarpc_fory::fory_service] compiles,
//! generates HelloService, registers types correctly, and round-trips
//! "hello world" over real fory TCP. No fork dependency.

use futures::StreamExt as _;
use tarpc::{
    client, context,
    server::{self, Channel},
};

#[tarpc::service(derive = [Clone, serde::Serialize, serde::Deserialize])]
#[tarpc_fory::fory_service]
trait Hello {
    async fn hello(name: String) -> String;
}

#[derive(Clone)]
struct HelloServer;

impl Hello for HelloServer {
    async fn hello(self, _: context::Context, name: String) -> String {
        format!("hello, {}", name)
    }
}

#[tokio::test]
async fn canonical_no_fork_round_trip() {
    // Server: listen with zero manual registration.
    let mut listener = tarpc_fory::listen::<HelloService, _>("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr();

    tokio::spawn(async move {
        while let Some(Ok(transport)) = listener.next().await {
            let channel = server::BaseChannel::with_defaults(transport);
            tokio::spawn(
                channel
                    .execute(HelloServer.serve())
                    .for_each(|fut| async move {
                        tokio::spawn(fut);
                    }),
            );
        }
    });

    // Client: connect with zero manual registration.
    let transport = tarpc_fory::connect::<HelloService, _>(addr).await.unwrap();
    let client = HelloClient::new(client::Config::default(), transport).spawn();

    let resp = client
        .hello(context::current(), "world".to_string())
        .await
        .unwrap();

    assert_eq!(resp, "hello, world");
}
```

- [ ] 10.5 Run the canonical test:

```bash
cd ~/work/fory-rpc/tarpc-fory && cargo test canonical_no_fork_round_trip 2>&1
```

Expected: `test canonical_no_fork_round_trip ... ok`

- [ ] 10.6 Commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add tests/canonical_no_fork.rs
git commit -m "test: canonical proof — no-fork round-trip over fory TCP"
```

---

## Task 11: Cleanup

- [ ] 11.1 Verify tarpc-fory's Cargo.toml has NO path dependency on tarpc-fork:

```bash
grep -n "tarpc-fork" ~/work/fory-rpc/tarpc-fory/Cargo.toml
```

Expected: no output.

- [ ] 11.2 Verify tarpc-fory does NOT import from `tarpc::serde_transport::fory` or `tarpc::serde_transport::fory_envelope` or `tarpc::serde_transport::fory_zerocopy` (these modules only exist in the fork):

```bash
grep -rn "tarpc::serde_transport::fory" ~/work/fory-rpc/tarpc-fory/src/ ~/work/fory-rpc/tarpc-fory/tests/
```

Expected: no output.

- [ ] 11.3 Remove the `#[cfg(feature = "fory")]` guards from the fork's proc-macro output. Since we no longer use the fork, this is informational only — the fork's code is dead.

- [ ] 11.4 Update `~/work/fory-rpc/tarpc-fory/README.md` — remove all references to "fork" and document the two-attribute pattern:

Replace the quick-start example with:
```rust
#[tarpc::service]
#[tarpc_fory::fory_service]
trait Hello {
    async fn hello(name: String) -> String;
}
```

Document that `tarpc-fory` depends on upstream `tarpc` 0.37 from crates.io.

- [ ] 11.5 Verify tokio-serde-fory is unaffected:

```bash
cd ~/work/fory-rpc/tokio-serde-fory && cargo test
```

Expected: all tests pass. tokio-serde-fory has no dependency on tarpc or tarpc-fork.

- [ ] 11.6 Final commit:

```bash
cd ~/work/fory-rpc/tarpc-fory
git add -A
git commit -m "chore: cleanup — remove all fork references, update README"
```

- [ ] 11.7 Push:

```bash
cd ~/work/fory-rpc/tarpc-fory && git push origin main
cd ~/work/fory-rpc/tarpc-fory-macros && git push origin main
```

---

## Summary

| Task | Description | Files | Est |
|------|-------------|-------|-----|
| 1 | Create tarpc-fory-macros crate skeleton | 2 new files | 15 min |
| 2 | Extract `#[fory_service]` proc-macro logic | 1 file (~350 LOC) | 3 hr |
| 3 | Move fory_envelope.rs to envelope.rs | 1 file (660 LOC) | 1 hr |
| 4 | Move fory.rs to transport.rs | 1 file (633 LOC) | 1 hr |
| 5 | Move fory_zerocopy.rs to zerocopy.rs | 1 file (1076 LOC) | 1 hr |
| 6 | Update tarpc-fory/src/lib.rs | 1 file | 30 min |
| 7 | Update tarpc-fory/Cargo.toml | 1 file | 30 min |
| 8 | Document fork decouple (no code change) | 0 files | 5 min |
| 9 | Move + update tests and benchmark | 9 test files + 1 bench | 2 hr |
| 10 | Full verification + canonical proof | 1 new test file | 1 hr |
| 11 | Cleanup + push | README + verify | 30 min |
| **Total** | | | **~11 hr** |

## Risk mitigation

| Risk | Mitigation |
|------|------------|
| `From<trace::SamplingDecision> for u8` orphan rule violation | Replace with helper function `sampling_to_u8()` (Task 3.5) |
| `tarpc::serde_transport::{new, Transport}` not public in upstream 0.37 | Verified: they are public. `Transport` is a `pub struct` and `new` is a `pub fn` in `serde_transport.rs`, part of the `serde-transport` feature |
| proc-macro can't parse tarpc's expanded output | Token stream structure is stable: trait + enum XxxRequest + enum XxxResponse. The parse strategy (wrap in module, find enums by name) is robust |
| Wire IDs change when code moves crates | Wire IDs use `module_path!()` evaluated in the user's crate (not in tarpc-fory). IDs do not change. |
| `serde` bounds on `Transport` (`Serialize`/`Deserialize`) | tarpc's `Transport` requires `SinkItem: Serialize` and `Item: Deserialize`. The serde derives on generated enums come from `#[tarpc::service(derive = [Serialize, Deserialize])]` — user must specify. The `ServiceWireSchema` + `connect`/`listen` API handles this via trait bounds. |
