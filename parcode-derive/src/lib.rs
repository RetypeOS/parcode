// parcode-derive/src/lib.rs

//! # Parcode Derive Macros
//!
//! This crate provides the procedural macros for `parcode`. It automates the implementation
//! of the `ParcodeVisitor`, `SerializationJob`, `ParcodeNative`, and the Lazy Mirror.
//!
//! Compatible with `syn 2.0`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, LitStr, parse_macro_input};

/// Derives `ParcodeVisitor`, `SerializationJob`, `ParcodeNative` and `ParcodeLazyRef`.
#[proc_macro_derive(ParcodeObject, attributes(parcode))]
pub fn derive_parcode_object(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    // 1. Validation
    let data_struct = match input.data {
        Data::Struct(ds) => ds,
        _ => {
            return syn::Error::new(name.span(), "ParcodeObject only supports structs")
                .to_compile_error()
                .into();
        }
    };

    // 2. Field Classification
    let mut locals = Vec::new();
    let mut remotes = Vec::new();

    for field in data_struct.fields {
        let (is_chunkable, compression_id) = match parse_attributes(&field.attrs) {
            Ok(res) => res,
            Err(e) => return e.to_compile_error().into(),
        };

        if is_chunkable {
            remotes.push(RemoteField {
                ident: field.ident.clone().unwrap(),
                ty: field.ty.clone(),
                compression_id,
            });
        } else {
            locals.push(LocalField {
                ident: field.ident.clone().unwrap(),
                ty: field.ty.clone(),
            });
        }
    }

    // 3. Code Generation
    let impl_visitor = generate_visitor(&name, &remotes, &locals);
    let impl_job = generate_serialization_job(&name, &locals);
    let impl_native = generate_native_reader(&name, &locals, &remotes);

    // NEW: Generate Lazy Mirror Struct and Trait Impl
    let impl_lazy = generate_lazy_mirror(&name, &locals, &remotes);

    // 4. Expansion
    let expanded = quote! {
        #impl_visitor
        #impl_job
        #impl_native
        #impl_lazy
    };

    TokenStream::from(expanded)
}

// --- Internal Data Structures (Same as before) ---
struct LocalField {
    ident: syn::Ident,
    ty: syn::Type,
}
struct RemoteField {
    ident: syn::Ident,
    ty: syn::Type,
    compression_id: u8,
}

// --- Parsing Logic (Same as before) ---
fn parse_attributes(attrs: &[Attribute]) -> syn::Result<(bool, u8)> {
    let mut is_chunkable = false;
    let mut compression_id = 0;
    for attr in attrs {
        if attr.path().is_ident("parcode") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("chunkable") {
                    is_chunkable = true;
                    return Ok(());
                }
                if meta.path.is_ident("compression") {
                    let value = meta.value()?;
                    let s: LitStr = value.parse()?;
                    compression_id = match s.value().to_lowercase().as_str() {
                        "lz4" => 1,
                        "zstd" => 2,
                        "none" => 0,
                        _ => return Err(meta.error("Unknown compression algorithm")),
                    };
                    return Ok(());
                }
                Err(meta.error("Unknown parcode attribute key"))
            })?;
        }
    }
    Ok((is_chunkable, compression_id))
}

// --- Generator: ParcodeVisitor (Same as before) ---
fn generate_visitor(
    name: &syn::Ident,
    remotes: &[RemoteField],
    _locals: &[LocalField],
) -> proc_macro2::TokenStream {
    let visit_children = remotes.iter().map(|f| {
        let fname = &f.ident;
        let cid = f.compression_id;
        let config_expr = if cid > 0 {
            quote! { Some(parcode::graph::JobConfig { compression_id: #cid }) }
        } else {
            quote! { None }
        };
        quote! { self.#fname.visit(graph, Some(my_id), #config_expr); }
    });

    quote! {
        impl parcode::visitor::ParcodeVisitor for #name {
            fn visit<'a>(&'a self, graph: &mut parcode::graph::TaskGraph<'a>, parent_id: Option<parcode::graph::ChunkId>, config_override: Option<parcode::graph::JobConfig>) {
                let job = self.create_job(config_override);
                let my_id = graph.add_node(job);
                if let Some(pid) = parent_id { graph.link_parent_child(pid, my_id); }
                #(#visit_children)*
            }
            fn create_job<'a>(&'a self, config_override: Option<parcode::graph::JobConfig>) -> Box<dyn parcode::graph::SerializationJob<'a> + 'a> {
                let base_job = Box::new(self.clone());
                if let Some(cfg) = config_override { Box::new(parcode::rt::ConfiguredJob::new(base_job, cfg)) } else { base_job }
            }
        }
    }
}

// --- Generator: SerializationJob (Same as before) ---
fn generate_serialization_job(
    name: &syn::Ident,
    locals: &[LocalField],
) -> proc_macro2::TokenStream {
    let serialize_stmts = locals.iter().map(|f| {
        let fname = &f.ident;
        quote! {
            parcode::internal::bincode::serde::encode_into_std_write(
                &self.#fname, &mut writer, parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    quote! {
        impl<'a> parcode::graph::SerializationJob<'a> for #name {
            fn execute(&self, _children_refs: &[parcode::format::ChildRef]) -> parcode::Result<Vec<u8>> {
                let mut buffer = Vec::new();
                let mut writer = std::io::BufWriter::new(&mut buffer);
                #(#serialize_stmts)*
                use std::io::Write;
                writer.flush()?;
                drop(writer);
                Ok(buffer)
            }
            fn estimated_size(&self) -> usize { std::mem::size_of::<Self>() }
        }
    }
}

// --- Generator: ParcodeNative (Same as before) ---
fn generate_native_reader(
    name: &syn::Ident,
    locals: &[LocalField],
    remotes: &[RemoteField],
) -> proc_macro2::TokenStream {
    let read_locals = locals.iter().map(|f| {
        let fname = &f.ident;
        let fty = &f.ty;
        quote! {
            let #fname: #fty = parcode::internal::bincode::serde::decode_from_std_read(
                &mut reader, parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    let read_remotes = remotes.iter().map(|f| {
        let fname = &f.ident;
        let fty = &f.ty;
        quote! {
            let child_node = child_iter.next().ok_or_else(|| parcode::ParcodeError::Format(format!("Missing child for '{}'", stringify!(#fname))))?;
            let #fname: #fty = parcode::reader::ParcodeNative::from_node(&child_node)?;
        }
    });

    let mut field_names = Vec::new();
    for f in locals {
        field_names.push(&f.ident);
    }
    for f in remotes {
        field_names.push(&f.ident);
    }

    quote! {
        impl parcode::reader::ParcodeNative for #name {
            fn from_node(node: &parcode::reader::ChunkNode<'_>) -> parcode::Result<Self> {
                let payload = node.read_raw()?;
                let mut reader = std::io::Cursor::new(payload);
                #(#read_locals)*
                let children = node.children()?;
                let mut child_iter = children.into_iter();
                #(#read_remotes)*
                Ok(Self { #(#field_names),* })
            }
        }
    }
}

// --- NEW GENERATOR: LAZY MIRROR ---

fn generate_lazy_mirror(
    name: &syn::Ident,
    locals: &[LocalField],
    remotes: &[RemoteField],
) -> proc_macro2::TokenStream {
    let lazy_name = syn::Ident::new(&format!("{}Lazy", name), name.span());

    // Fields definition for the Lazy struct
    let lazy_fields = locals
        .iter()
        .map(|f| {
            let n = &f.ident;
            let t = &f.ty;
            quote! { pub #n: #t } // Locals are stored directly
        })
        .chain(remotes.iter().map(|f| {
            let n = &f.ident;
            let t = &f.ty;
            // Recursive Lazy Type resolution
            quote! { pub #n: <#t as parcode::rt::ParcodeLazyRef<'a>>::Lazy }
        }));

    // Logic to read locals (Same as Native Reader)
    let read_locals_stmts = locals.iter().map(|f| {
        let n = &f.ident;
        let t = &f.ty;
        quote! {
            let #n: #t = parcode::internal::bincode::serde::decode_from_std_read(
                &mut reader, parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    // Logic to wrap remotes (Call create_lazy instead of deserializing)
    let assign_remotes_stmts = remotes.iter().map(|f| {
        let n = &f.ident;
        let t = &f.ty;
        quote! {
            let child = child_iter.next().ok_or(parcode::ParcodeError::Format("Missing child".into()))?;
            let #n = <#t as parcode::rt::ParcodeLazyRef<'a>>::create_lazy(child)?;
        }
    });

    let mut field_names = Vec::new();
    for f in locals {
        field_names.push(&f.ident);
    }
    for f in remotes {
        field_names.push(&f.ident);
    }

    quote! {
        /// Generated Lazy Mirror for #name.
        #[derive(Debug)]
        pub struct #lazy_name<'a> {
            #(#lazy_fields),*
        }

        impl<'a> parcode::rt::ParcodeLazyRef<'a> for #name {
            type Lazy = #lazy_name<'a>;

            fn create_lazy(node: parcode::reader::ChunkNode<'a>) -> parcode::Result<Self::Lazy> {
                // 1. Eager Load Locals
                let payload = node.read_raw()?;
                let mut reader = std::io::Cursor::new(payload);
                #(#read_locals_stmts)*

                // 2. Lazy Wrap Remotes
                let children = node.children()?;
                let mut child_iter = children.into_iter();
                #(#assign_remotes_stmts)*

                Ok(#lazy_name {
                    #(#field_names),*
                })
            }
        }
    }
}
