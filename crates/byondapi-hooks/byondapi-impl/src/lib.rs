use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use syn::{spanned::Spanned, Lit};

fn extract_args(a: &syn::FnArg) -> &syn::PatType {
	match a {
		syn::FnArg::Typed(p) => p,
		_ => panic!("Not supported on types with `self`!"),
	}
}

#[proc_macro_attribute]
pub fn bind(attr: TokenStream, item: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(item as syn::ItemFn);
	let proc = syn::parse_macro_input!(attr as Option<syn::Lit>);

	let func_name = &input.sig.ident;
	let func_name_disp = quote!(#func_name).to_string();

	let func_name_ffi = format!("{func_name_disp}_ffi");
	let func_name_ffi = Ident::new(&func_name_ffi, func_name.span());
	let func_name_ffi_disp = quote!(#func_name_ffi).to_string();

	let args = &input.sig.inputs;

	//Check for returns
	match &input.sig.output {
		syn::ReturnType::Default => {} //

		syn::ReturnType::Type(_, ty) => {
			return syn::Error::new(ty.span(), "Do not specify the return value of hooks")
				.to_compile_error()
				.into()
		}
	}

	let signature = quote! {
		#[no_mangle]
		pub unsafe extern "C" fn #func_name_ffi (
			__argc: ::byondapi::sys::u4c,
			__argv: *mut ::byondapi::value::ByondValue
		) -> ::byondapi::value::ByondValue
	};

	let body = &input.block;
	let mut arg_names: syn::punctuated::Punctuated<syn::Ident, syn::Token![,]> =
		syn::punctuated::Punctuated::new();
	let mut proc_arg_unpacker: syn::punctuated::Punctuated<
		proc_macro2::TokenStream,
		syn::Token![,],
	> = syn::punctuated::Punctuated::new();

	for arg in args.iter().map(extract_args) {
		if let syn::Pat::Ident(p) = &*arg.pat {
			arg_names.push(p.ident.clone());
			let index = arg_names.len() - 1;
			proc_arg_unpacker.push(
				(quote! {
					args.get(#index).map(::byondapi::value::ByondValue::clone).unwrap_or_default()
				})
				.into(),
			);
		}
	}

	let arg_names_disp = quote!(#arg_names).to_string();

	//Submit to inventory
	let cthook_prelude = match &proc {
		Some(Lit::Str(p)) => {
			quote! {
				::byondapi_hooks::inventory::submit!({
					::byondapi_hooks::Bind {
						proc_path: #p,
						func_name: #func_name_ffi_disp,
						func_arguments: Some(#arg_names_disp)
					}
				});
			}
		}
		Some(other_literal) => {
			return syn::Error::new(
				other_literal.span(),
				"Bind attributes must be a string literal or empty",
			)
			.to_compile_error()
			.into()
		}
		None => quote! {
			::byondapi_hooks::inventory::submit!({
				::byondapi_hooks::Bind{
					proc_path: #func_name_disp,
					func_name: #func_name_ffi_disp,
					func_arguments: Some(#arg_names_disp)
				}
			});
		},
	};

	let result = quote! {
		#cthook_prelude
		#signature {
			let args = unsafe { ::byondapi::parse_args(__argc, __argv) };
			match #func_name(#proc_arg_unpacker) {
				Ok(val) => val,
				Err(e) => {
					let error_string = ::byondapi::value::ByondValue::try_from(::std::format!("{e:?}")).unwrap();
					::byondapi::global_call::call_global("stack_trace", &[error_string]).unwrap();
					::byondapi::value::ByondValue::null()
				}
			}

		}
		fn #func_name(#args) -> ::eyre::Result<::byondapi::value::ByondValue>
		#body
	};
	result.into()
}

#[proc_macro_attribute]
pub fn bind_raw_args(attr: TokenStream, item: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(item as syn::ItemFn);
	let proc = syn::parse_macro_input!(attr as Option<syn::Lit>);

	let func_name = &input.sig.ident;
	let func_name_disp = quote!(#func_name).to_string();

	let func_name_ffi = format!("{func_name_disp}_ffi");
	let func_name_ffi = Ident::new(&func_name_ffi, func_name.span());
	let func_name_ffi_disp = quote!(#func_name_ffi).to_string();

	//Check for returns
	match &input.sig.output {
		syn::ReturnType::Default => {} //

		syn::ReturnType::Type(_, ty) => {
			return syn::Error::new(ty.span(), "Do not specify the return value of binds")
				.to_compile_error()
				.into()
		}
	}

	if !input.sig.inputs.is_empty() {
		return syn::Error::new(
			input.sig.inputs.span(),
			"Do not specify arguments for raw arg binds",
		)
		.to_compile_error()
		.into();
	}

	let signature = quote! {
		pub unsafe extern "C" fn #func_name_ffi (
			__argc: ::byondapi::sys::u4c,
			__argv: *mut ::byondapi::value::ByondValue
		) -> ::byondapi::value::ByondValue
	};

	let body = &input.block;

	//Submit to inventory
	let cthook_prelude = match proc {
		Some(Lit::Str(p)) => {
			quote! {
				::byondapi_hooks::inventory::submit!({
					::byondapi_hooks::Bind {
						proc_path: #p,
						func_name: #func_name_ffi_disp,
						func_arguments: None
					}
				});
			}
		}
		Some(other_literal) => {
			return syn::Error::new(
				other_literal.span(),
				"Bind attributes must be a string literal or empty",
			)
			.to_compile_error()
			.into()
		}
		None => quote! {
			quote! {
				::byondapi_hooks::inventory::submit!({
					::byondapi_hooks::Bind{
						proc_path: #func_name_disp,
						func_name: #func_name_ffi_disp,
						func_arguments: None,
					}
				});

			}
		},
	};

	let result = quote! {
		#cthook_prelude
		#signature {
			let mut args = unsafe { ::byondapi::parse_args(__argc, __argv) };
			match #func_name(args) {
				Ok(val) => val,
				Err(e) => {
					let error_string = ::byondapi::value::ByondValue::try_from(::std::format!("{e:?}")).unwrap();
					::byondapi::global_call::call_global("stack_trace", &[error_string]).unwrap();
					::byondapi::value::ByondValue::null()
				}
			}

		}
		fn #func_name(args: &mut [::byondapi::value::ByondValue]) -> ::eyre::Result<::byondapi::value::ByondValue>
		#body
	};
	result.into()
}
