
use proc_macro::TokenStream;
use quote::quote;

#[proc_macro_attribute]
pub fn datum_del(_: TokenStream, item: TokenStream) -> TokenStream {
	let func = syn::parse_macro_input!(item as syn::ItemFn);
	let func_name = &func.sig.ident;

	let inventory_define = quote! {
		inventory::submit!(
			DelDatumFunc(#func_name)
		);
	};

	let code = quote! {
		#func
		#inventory_define
	};

	code.into()
}