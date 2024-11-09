use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::spanned::Spanned;

fn strip_mut_and_filter(arg: &syn::FnArg) -> Option<syn::FnArg> {
	let syn::FnArg::Typed(pattype) = arg else {
		return None;
	};
	let mut ident_clone = pattype.clone();

	match &mut *ident_clone.pat {
		syn::Pat::Ident(p) => {
			p.mutability = None;
			Some(syn::FnArg::Typed(ident_clone))
		}
		syn::Pat::Tuple(tuple) => {
			tuple.elems.iter_mut().for_each(|item| {
				let syn::Pat::Ident(item) = item else { return };
				item.mutability = None;
			});
			Some(syn::FnArg::Typed(ident_clone))
		}
		_ => Some(syn::FnArg::Typed(ident_clone)),
	}
}

/// This macros generates simd versions of functions as well as regular ones,
/// allowing these functions to run in cpus without the required instructions.
/// The specific simd feature used here is avx2.
/// Example usage:
/// ```
///#[auxmacros::generate_simd_functions]
///#[byondapi::bind("/proc/process_atmos_callbacks")]
///fn atmos_callback_handle(remaining: ByondValue) -> Result<ByondValue> {
///	auxcallback::callback_processing_hook(remaining)
///}
/// ```
#[proc_macro_attribute]
pub fn generate_simd_functions(
	_: proc_macro::TokenStream,
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let input = syn::parse_macro_input!(item as syn::ItemFn);

	let attrs = input
		.attrs
		.into_iter()
		.map(|attr| quote! { #attr })
		.collect::<TokenStream>();

	let func_name = &input.sig.ident;
	let func_name_disp = quote!(#func_name).to_string();
	let func_name_simd = format!("{func_name_disp}_simd");
	let func_ident_simd = Ident::new(&func_name_simd, func_name.span());
	let func_name_fallback = format!("{func_name_disp}_fallback");
	let func_ident_fallback = Ident::new(&func_name_fallback, func_name.span());

	let args = &input.sig.inputs;
	let body = input.block;
	let func_return = input.sig.output;

	if let Some(recv) = args
		.iter()
		.find(|item| matches!(item, syn::FnArg::Receiver(_)))
	{
		return syn::Error::new(recv.span(), "Self is not supported!")
			.to_compile_error()
			.into();
	}

	let args_nonmut = args
		.iter()
		.filter_map(strip_mut_and_filter)
		.map(|item| quote! {#item})
		.collect::<syn::punctuated::Punctuated<TokenStream, syn::Token![,]>>();

	let args_typeless = args
		.iter()
		.filter_map(strip_mut_and_filter)
		.filter_map(|arg| {
			let syn::FnArg::Typed(pattype) = arg else {
				return None;
			};
			let pattype = &*pattype.pat;
			Some(quote! {#pattype})
		})
		.collect::<syn::punctuated::Punctuated<TokenStream, syn::Token![,]>>();

	quote! {
		#attrs
		fn #func_name(#args_nonmut) #func_return {
			// This `unsafe` block is safe because we're testing
			// that the `avx2` feature is indeed available on our CPU.
			if *crate::_SIMD_DETECTED.get_or_init(|| is_x86_feature_detected!("avx2")) {
				unsafe { #func_ident_simd(#args_typeless) }
			} else {
				#func_ident_fallback(#args_typeless)
			}
		}

		#[target_feature(enable = "avx2")]
		unsafe fn #func_ident_simd(#args_nonmut) #func_return {
			#func_ident_fallback(#args_typeless)
		}

		#[inline(always)]
		fn #func_ident_fallback(#args) #func_return
		#body
	}
	.into()
}
