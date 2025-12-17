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

    let data_struct = match input.data {
        Data::Struct(ds) => ds,
        _ => {
            return syn::Error::new(name.span(), "ParcodeObject only supports structs")
                .to_compile_error()
                .into();
        }
    };

    let mut locals = Vec::new();
    let mut remotes = Vec::new();

    for field in data_struct.fields {
        // Parse attributes.
        let (is_chunkable, compression_id, is_map) = match parse_attributes(&field.attrs) {
            Ok(res) => res,
            Err(e) => return e.to_compile_error().into(),
        };

        // If it's 'map', it's implicitly chunkable (it's a child node).
        if is_chunkable || is_map {
            remotes.push(RemoteField {
                ident: field.ident.clone().unwrap(),
                ty: field.ty.clone(),
                compression_id,
                is_map,
            });
        } else {
            locals.push(LocalField {
                ident: field.ident.clone().unwrap(),
                ty: field.ty.clone(),
            });
        }
    }

    let impl_visitor = generate_visitor(&name, &remotes, &locals);
    let impl_job = generate_serialization_job(&name, &locals);
    let impl_native = generate_native_reader(&name, &locals, &remotes);
    let impl_item = generate_parcode_item(&name, &locals, &remotes);
    let impl_lazy = generate_lazy_mirror(&name, &locals, &remotes);

    let expanded = quote! {
        #impl_visitor
        #impl_job
        #impl_native
        #impl_item
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
    is_map: bool,
}

/// Parses attributes. Returns (is_chunkable, compression_id, is_map).
fn parse_attributes(attrs: &[Attribute]) -> syn::Result<(bool, u8, bool)> {
    let mut is_chunkable = false;
    let mut is_map = false;
    let mut compression_id = 0;

    for attr in attrs {
        if attr.path().is_ident("parcode") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("chunkable") {
                    is_chunkable = true;
                    return Ok(());
                }

                if meta.path.is_ident("map") {
                    is_map = true;
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
                Err(meta
                    .error("Unknown parcode attribute key. Supported: chunkable, map, compression"))
            })?;
        }
    }
    Ok((is_chunkable, compression_id, is_map))
}

// --- Generator: ParcodeVisitor ---

fn generate_visitor(
    name: &syn::Ident,
    remotes: &[RemoteField],
    _locals: &[LocalField],
) -> proc_macro2::TokenStream {
    let visit_children_standalone = remotes.iter().map(|f| {
        let fname = &f.ident;
        let cid = f.compression_id;
        let is_map = f.is_map;

        let config_expr = if cid > 0 || is_map {
            quote! {
                Some(parcode::graph::JobConfig {
                    compression_id: #cid,
                    is_map: #is_map
                })
            }
        } else {
            quote! { None }
        };

        quote! { self.#fname.visit(graph, Some(my_id), #config_expr); }
    });

    let visit_children_inlined = remotes.iter().map(|f| {
        let fname = &f.ident;
        let cid = f.compression_id;
        let is_map = f.is_map;

        let config_expr = if cid > 0 || is_map {
            quote! {
                Some(parcode::graph::JobConfig {
                    compression_id: #cid,
                    is_map: #is_map
                })
            }
        } else {
            quote! { None }
        };

        quote! { self.#fname.visit(graph, Some(pid), #config_expr); }
    });

    // Generate serialization statements for locals (reused from SerializationJob logic)
    let serialize_locals = _locals.iter().map(|f| {
        let fname = &f.ident;
        quote! {
            parcode::internal::bincode::serde::encode_into_std_write(
                &self.#fname, writer, parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    quote! {
        impl parcode::visitor::ParcodeVisitor for #name {
            fn visit<'a>(&'a self, graph: &mut parcode::graph::TaskGraph<'a>, parent_id: Option<parcode::graph::ChunkId>, config_override: Option<parcode::graph::JobConfig>) {
                // Standard Visit: Create a node for self.
                let job = self.create_job(config_override);
                let my_id = graph.add_node(job);
                if let Some(pid) = parent_id {
                    graph.link_parent_child(pid, my_id);
                }
                #(#visit_children_standalone)*
            }

            fn visit_inlined<'a>(&'a self, graph: &mut parcode::graph::TaskGraph<'a>, pid: parcode::graph::ChunkId, _config_override: Option<parcode::graph::JobConfig>) {
                #(#visit_children_inlined)*
            }

            fn create_job<'a>(&'a self, config_override: Option<parcode::graph::JobConfig>) -> Box<dyn parcode::graph::SerializationJob<'a> + 'a> {
                let base_job = Box::new(self.clone());
                if let Some(cfg) = config_override { Box::new(parcode::rt::ConfiguredJob::new(base_job, cfg)) } else { base_job }
            }

            fn serialize_shallow<W: std::io::Write>(&self, writer: &mut W) -> parcode::Result<()>
            where
                Self: serde::Serialize,
            {
                #(#serialize_locals)*
                Ok(())
            }

            fn serialize_slice<W: std::io::Write>(slice: &[Self], writer: &mut W) -> parcode::Result<()>
            where
                Self: Sized + serde::Serialize,
            {
                let len = slice.len() as u64;
                parcode::internal::bincode::serde::encode_into_std_write(
                    &len,
                    writer,
                    parcode::internal::bincode::config::standard(),
                ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;

                for item in slice {
                    item.serialize_shallow(writer)?;
                }
                Ok(())
            }
        }
    }
}

// --- Generator: SerializationJob ---
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

// --- Generator: ParcodeItem ---
fn generate_parcode_item(
    name: &syn::Ident,
    locals: &[LocalField],
    remotes: &[RemoteField],
) -> proc_macro2::TokenStream {
    let read_locals = locals.iter().map(|f| {
        let fname = &f.ident;
        let fty = &f.ty;
        quote! {
            let #fname: #fty = parcode::internal::bincode::serde::decode_from_std_read(
                reader, parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    let read_remotes = remotes.iter().map(|f| {
        let fname = &f.ident;
        let fty = &f.ty;
        quote! {
            let child_node = children.next().ok_or_else(|| parcode::ParcodeError::Format(format!("Missing child for '{}'", stringify!(#fname))))?;
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
        impl parcode::reader::ParcodeItem for #name {
            fn read_from_shard(
                reader: &mut std::io::Cursor<&[u8]>,
                children: &mut std::vec::IntoIter<parcode::reader::ChunkNode<'_>>,
            ) -> parcode::Result<Self> {
                #(#read_locals)*
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

    let lazy_fields_def = locals
        .iter()
        .map(|f| {
            let n = &f.ident;
            let t = &f.ty;
            quote! { pub #n: #t }
        })
        .chain(remotes.iter().map(|f| {
            let n = &f.ident;
            let t = &f.ty;
            quote! { pub #n: <#t as parcode::rt::ParcodeLazyRef<'a>>::Lazy }
        }));

    // Logic to read from stream (reusable block)
    let read_logic = {
        let read_locals = locals.iter().map(|f| {
            let n = &f.ident;
            let t = &f.ty;
            quote! {
                let #n: #t = parcode::internal::bincode::serde::decode_from_std_read(
                    &mut reader, parcode::internal::bincode::config::standard()
                ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
            }
        });

        let assign_remotes = remotes.iter().map(|f| {
            let n = &f.ident;
            let t = &f.ty;
            // This handles nested Vecs or nested Structs correctly.
            quote! {
                let #n = <#t as parcode::rt::ParcodeLazyRef<'a>>::read_lazy_from_stream(reader, child_iter)?;
            }
        });

        let field_names = locals
            .iter()
            .map(|f| &f.ident)
            .chain(remotes.iter().map(|f| &f.ident));

        quote! {
            #(#read_locals)*
            #(#assign_remotes)*

            Ok(#lazy_name {
                #(#field_names,)*
                _marker: std::marker::PhantomData,
            })
        }
    };

    quote! {
        #[derive(Debug)]
        pub struct #lazy_name<'a> {
            #(#lazy_fields_def,)*
            _marker: std::marker::PhantomData<&'a ()>,
        }

        impl<'a> parcode::rt::ParcodeLazyRef<'a> for #name {
            type Lazy = #lazy_name<'a>;

            fn create_lazy(node: parcode::reader::ChunkNode<'a>) -> parcode::Result<Self::Lazy> {
                let payload = node.read_raw()?;
                let mut reader = std::io::Cursor::new(payload.as_ref());
                let children = node.children()?;
                let mut child_iter = children.into_iter();

                Self::read_lazy_from_stream(&mut reader, &mut child_iter)
            }

            fn read_lazy_from_stream(
                mut reader: &mut std::io::Cursor<&[u8]>,
                mut child_iter: &mut std::vec::IntoIter<parcode::reader::ChunkNode<'a>>
            ) -> parcode::Result<Self::Lazy> {
                #read_logic
            }
        }
    }
}
