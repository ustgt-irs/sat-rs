use quote::{ToTokens, format_ident, quote};
use syn::{ItemConst, LitStr, parse_macro_input};

/// This macro can be used to automatically generate introspection information for return codes.
///
/// For example, it can be applied to types like the
/// [`satrs_mib::res_code::ResultU16`](https://docs.rs/satrs-mib/latest/satrs_mib/res_code/struct.ResultU16.html#) type
/// to automatically generate
/// [`satrs_mib::res_code::ResultU16Info`](https://docs.rs/satrs-mib/latest/satrs_mib/res_code/struct.ResultU16Info.html)
/// instances. These instances can then be used for tasks like generating CSVs or YAML files with
/// the list of all result codes. This information is valuable for both operators and developers.
#[proc_macro_attribute]
pub fn resultcode(
    args: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // Handle attributes first.
    let mut info_str: Option<LitStr> = None;
    let res_code_parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("info") {
            info_str = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("unsupported property for resultcode attribute"))
        }
    });
    parse_macro_input!(args with res_code_parser);
    let item = parse_macro_input!(item as ItemConst);

    // Generate additional generated info struct used for introspection.
    let result_code_name = &item.ident;
    let name_as_str = result_code_name.to_string();
    let gen_struct_name = format_ident!("{}_EXT", result_code_name);
    let info_str = info_str.map_or(String::from(""), |v| v.value());
    // TODO: Group string
    let generated_struct = quote! {
        const #gen_struct_name: satrs_mib::res_code::ResultU16Info =
            satrs_mib::res_code::ResultU16Info::const_new(
                #name_as_str,
                &#result_code_name,
                "",
                #info_str
            );
    };

    // The input constant returncode is written to the output in any case.
    let mut output = item.to_token_stream();
    output.extend(generated_struct);
    output.into()
}
