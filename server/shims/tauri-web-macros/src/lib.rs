//! `#[tauri::command]` replacement for the headless web build.
//!
//! Keeps the annotated function as-is and emits a sibling
//! `__web_cmd_<name>(app: AppHandle, args: Value) -> BoxFuture<InvokeResult>`
//! that deserializes camelCase args (Tauri's default), injects
//! `AppHandle` / `State<T>` parameters, runs the command (sync commands on
//! the blocking pool, like Tauri runs them off the main thread) and
//! serializes `Ok`/`Err` to JSON.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ItemFn, Pat, ReturnType, Type};

fn last_segment(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(r) => last_segment(&r.elem),
        _ => None,
    }
}

fn camel_case(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for (i, ch) in name.chars().enumerate() {
        if ch == '_' {
            if i > 0 {
                upper = true;
            }
            continue;
        }
        if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    let rename_snake = attr.to_string().contains("snake_case");
    let func = parse_macro_input!(item as ItemFn);
    let name = &func.sig.ident;
    let name_str = name.to_string();
    let wrapper = format_ident!("__web_cmd_{}", name);
    let vis = &func.vis;
    let is_async = func.sig.asyncness.is_some();

    let mut lets = Vec::new();
    let mut call_args = Vec::new();
    for (idx, input) in func.sig.inputs.iter().enumerate() {
        let FnArg::Typed(pt) = input else {
            return syn::Error::new_spanned(input, "web shim: methods are not supported as commands")
                .to_compile_error()
                .into();
        };
        let var = format_ident!("__a{}", idx);
        let arg_name = match &*pt.pat {
            Pat::Ident(id) => id.ident.to_string(),
            _ => format!("arg{idx}"),
        };
        let arg_name = arg_name.trim_start_matches('_').to_string();
        let key = if rename_snake {
            arg_name.clone()
        } else {
            camel_case(&arg_name)
        };
        let seg = last_segment(&pt.ty).unwrap_or_default();
        let ty = &pt.ty;
        match seg.as_str() {
            "AppHandle" => {
                lets.push(quote! { let #var = __app.clone(); });
            }
            "State" => {
                lets.push(quote! { let #var = ::tauri::__private::state_owned(&__app)?; });
            }
            "Window" | "WebviewWindow" | "Webview" => {
                return syn::Error::new_spanned(ty, "web shim: window parameters are not supported")
                    .to_compile_error()
                    .into();
            }
            _ => {
                lets.push(quote! {
                    let #var: #ty = ::tauri::__private::arg(&__args, #key, #name_str)?;
                });
            }
        }
        call_args.push(var);
    }

    let returns_result = match &func.sig.output {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => last_segment(ty).as_deref() == Some("Result"),
    };

    let convert = if returns_result {
        quote! {
            match __r {
                Ok(v) => ::tauri::__private::ok_value(v),
                Err(e) => ::tauri::__private::err_value(e),
            }
        }
    } else {
        quote! { ::tauri::__private::ok_value(__r) }
    };

    let invoke = if is_async {
        quote! {
            let __r = #name(#(#call_args),*).await;
            #convert
        }
    } else {
        quote! {
            let __r = ::tauri::async_runtime::spawn_blocking(move || #name(#(#call_args),*))
                .await
                .map_err(::tauri::__private::join_error)?;
            #convert
        }
    };

    let expanded = quote! {
        #func

        #[doc(hidden)]
        #[allow(non_snake_case, unused_variables, clippy::all)]
        #vis fn #wrapper(
            __app: ::tauri::AppHandle,
            __args: ::tauri::__private::Value,
        ) -> ::tauri::__private::BoxFuture<'static, ::tauri::__private::InvokeResult> {
            ::std::boxed::Box::pin(async move {
                #(#lets)*
                #invoke
            })
        }
    };
    expanded.into()
}
