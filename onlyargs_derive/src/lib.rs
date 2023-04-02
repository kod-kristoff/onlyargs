//! Derive macro for [`onlyargs`](https://docs.rs/onlyargs).
//!
//! The parser generated by this macro is very opinionated. The implementation attempts to be as
//! light as possible while also being usable for most applications.
//!
//! Only structs with named fields are supported. Doc comments are used for the generated help text.
//! Argument names are generated automatically from field names with only a few rules:
//!
//! - Long argument names start with `--`, ASCII alphabetic characters are made lowercase, and all
//!   `_` characters are replaced with `-`.
//! - Short argument names use the first ASCII alphabetic character of the field name following a
//!   `-`. Short arguments are not allowed to be duplicated.
//!   - This behavior can be suppressed with the `#[long]` attribute (see below). This attribute
//!     can be used on the `version` field to allow `-v` to reference the `verbose` field.
//!   - Alternatively, the `#[short('…')]` attribute can be used to set a specific short name.
//!
//! # Provided arguments
//!
//! By default, `--help` and `--version` arguments are automatically generated. When the parser
//! encounters either, it will print the help or version message and exit the application with exit
//! code 0. (**TODO**): This behavior can be suppressed with an extra attribute on the struct:
//!
//! ```ignore
//! #[derive(Debug, OnlyArgs)]
//! #[onlyargs(help = true, version = false, short_help = false)]
//! struct Args {
//!     verbose: bool,
//! }
//! ```
//!
//! # Attributes
//!
//! Parsing options are configurable with the following attributes:
//!
//! - `#[long]` (**TODO**): Only generate long argument names like `--help`. Short args like `-h`
//!   are generated by default, and this attribute suppresses that behavior.
//! - `#[short('N')]` (**TODO**): Generate a short argument name with the given character. In this
//!   example, it will be `-N`.
//! - `#[default(T)]` (**TODO**): Specify a default value for an argument.
//!
//! # Supported types
//!
//! Here is the list of supported field "primitive" types:
//!
//! | Type          | Description                                      |
//! |---------------|--------------------------------------------------|
//! | `bool`        | Defines a flag.                                  |
//! | `f32|f64`     | Floating point number option.                    |
//! | `i8|u8`       | 8-bit integer option.                            |
//! | `i16|u16`     | 16-bit integer option.                           |
//! | `i32|u32`     | 32-bit integer option.                           |
//! | `i64|u64`     | 64-bit integer option.                           |
//! | `i128|u128`   | 128-bit integer option.                          |
//! | `isize|usize` | Pointer-sized integer option.                    |
//! | `OsString`    | A string option with platform-specific encoding. |
//! | `PathBuf`     | A file system path option.                       |
//! | `String`      | UTF-8 encoded string option.                     |
//!
//! Additionally, some wrapper and composite types are also available, where the type `T` must be
//! one of the primitive types listed above.
//!
//! | Type          | Description                       |
//! |---------------|-----------------------------------|
//! | `Option<T>`   | An optional argument.             |
//! | `Vec<T>`      | Positional arguments (see below). |
//!
//! In argument parsing parlance, "flags" are simple boolean values; the argument does not require
//! a value. For example, the argument `--help`. This concept is distinct from options with optional
//! values.
//!
//! "Options" carry a value and the argument parser requires the value to directly follow the
//! argument name. Option values can be made optional with `Option<T>`.
//!
//! ## Positional arguments
//!
//! If the struct contains a field with a vector type, it _must_ be the only vector field. This
//! becomes the "dumping ground" for all positional arguments, which are any args that do not match
//! an existing field, or any arguments following the `--` "stop parsing" sentinel.

// TODO: Redo this whole thing without `quote` and `syn` to optimize compile-time.
use crate::parser::*;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashMap;
use syn::{parse_macro_input, parse_quote, Ident};

mod parser;

/// See the [root module documentation](crate) for the DSL specification.
#[proc_macro_derive(OnlyArgs)]
pub fn derive_parser(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as ArgumentStruct);

    let mut flags = vec![
        ArgFlag {
            name: parse_quote!(help),
            short: Some('h'),
            doc: vec!["Show this help message.".to_string()],
            output: false,
        },
        ArgFlag {
            name: parse_quote!(version),
            short: Some('V'),
            doc: vec!["Show the application version.".to_string()],
            output: false,
        },
    ];
    flags.extend(ast.flags.into_iter());

    // De-dupe short args.
    let mut dupes = HashMap::new();
    for flag in &flags {
        if let Err(err) = dedupe(&mut dupes, flag.as_view()) {
            return err.into_compile_error().into();
        }
    }
    for opt in &ast.options {
        if let Err(err) = dedupe(&mut dupes, opt.as_view()) {
            return err.into_compile_error().into();
        }
    }

    // Produce help text for all arguments.
    let max_width = get_max_width(flags.iter().map(|arg| arg.as_view()));
    let flags_help = flags.iter().map(|arg| to_help(arg.as_view(), max_width));

    let max_width = get_max_width(ast.options.iter().map(|arg| arg.as_view()));
    let options_help = ast
        .options
        .iter()
        .map(|arg| to_help(arg.as_view(), max_width));

    let positional_header = match ast.positional.as_ref() {
        Some(opt) => vec![format!(" {}...", opt.name)],
        None => vec![],
    };
    let positional_help = match ast.positional.as_ref() {
        Some(opt) => vec![format!("{}:\n  ", opt.name), opt.doc.join("\n  ")],
        None => vec![],
    };

    // Produce variables for argument parser state.
    let flags_vars = flags.iter().filter_map(|flag| {
        flag.output.then(|| {
            let name = &flag.name;
            quote! { let mut #name = false; }
        })
    });
    let options_vars = ast.options.iter().map(|opt| {
        let name = &opt.name;
        quote! { let mut #name = None; }
    });
    let positional_var = match ast.positional.as_ref() {
        Some(opt) => {
            let name = &opt.name;
            vec![quote! { let mut #name = vec![]; }]
        }
        None => vec![],
    };

    // Produce matchers for parser.
    let flags_matchers = flags.iter().filter_map(|flag| {
        flag.output.then(|| {
            let name = &flag.name;
            let short = flag.short.map(|ch| {
                let arg = format!("-{ch}");
                quote! { | Some(#arg) }
            });
            let arg = format!("--{}", to_arg_name(name));

            quote! {
                Some(#arg) #short => {
                    #name = true;
                }
            }
        })
    });
    let options_matchers = ast.options.iter().map(|opt| {
        let name = &opt.name;
        let short = opt.short.map(|ch| {
            let arg = format!("-{ch}");
            quote! { | Some(name @ #arg) }
        });
        let arg = format!("--{}", to_arg_name(name));
        let value = match opt.ty_help {
            ArgType::Bool => unreachable!(),
            ArgType::Number => quote! { Some(args.next().parse_int(name)?) },
            ArgType::OsString => quote! { Some(args.next().parse_osstr(name)?) },
            ArgType::Path => quote! { Some(args.next().parse_path(name)?) },
            ArgType::String => quote! { Some(args.next().parse_str(name)?) },
        };

        quote! {
            Some(name @ #arg) #short => {
                #name = #value;
            }
        }
    });
    let positional_matcher = match ast.positional.as_ref() {
        Some(opt) => {
            let name = &opt.name;
            let value = match opt.ty_help {
                ArgType::Bool => unreachable!(),
                ArgType::Number => quote! { arg.parse_int("<POSITIONAL>")? },
                ArgType::OsString => quote! { arg.parse_osstr("<POSITIONAL>")? },
                ArgType::Path => quote! { arg.parse_path("<POSITIONAL>")? },
                ArgType::String => quote! { arg.parse_str("<POSITIONAL>")? },
            };

            vec![quote! {
                Some("--") => {
                    for arg in args {
                        #name.push(#value);
                    }
                    break;
                }
                Some(_) => {
                    #name.push(#value);
                }
            }]
        }
        None => vec![quote! { Some("--") => break, }],
    };

    // Produce identifiers for args constructor.
    let flags_idents = flags
        .iter()
        .filter_map(|flag| flag.output.then_some(&flag.name));
    let options_idents = ast.options.iter().map(|opt| {
        let name = &opt.name;
        let arg = format!("--{}", to_arg_name(name));
        if opt.optional {
            quote! { #name }
        } else {
            quote! { #name: #name.required(#arg)? }
        }
    });
    let positional_ident = match ast.positional.as_ref() {
        Some(opt) => vec![&opt.name],
        None => vec![],
    };

    let name = ast.name;
    let doc_comment = ast.doc.join("\n");

    // Produce final code.
    let code = quote! {
        impl ::onlyargs::OnlyArgs for #name {
            const HELP: &'static str = concat!(
                env!("CARGO_PKG_NAME"),
                " v",
                env!("CARGO_PKG_VERSION"),
                "\n",
                env!("CARGO_PKG_DESCRIPTION"),
                "\n\n",
                #doc_comment,
                "\n\nUsage:\n  ",
                env!("CARGO_BIN_NAME"),
                " [flags] [options]",
                #(#positional_header,)*
                "\n\nFlags:\n",
                #(#flags_help,)*
                "\nOptions:\n",
                #(#options_help,)*
                #(#positional_help,)*
                "\n",
            );

            fn parse(args: Vec<std::ffi::OsString>) -> Result<Self, ::onlyargs::CliError> {
                use ::onlyargs::extensions::*;

                #(#flags_vars)*
                #(#options_vars)*
                #(#positional_var)*

                let mut args = args.into_iter();
                while let Some(arg) = args.next() {
                    match arg.to_str() {
                        // TODO: Add an attribute to disable help/version.
                        Some("--help") | Some("-h") => Self::help(),
                        Some("--version") | Some("-V") => Self::version(),
                        #(#flags_matchers)*
                        #(#options_matchers)*
                        #(#positional_matcher)*
                        _ => return Err(::onlyargs::CliError::Unknown(arg)),
                    }
                }

                Ok(Self {
                    #(#flags_idents,)*
                    #(#options_idents,)*
                    #(#positional_ident,)*
                })
            }
        }
    };

    code.into()
}

// 1 hyphen + 1 char + 1 trailing space.
const SHORT_PAD: usize = 3;
// 2 leading spaces + 2 hyphens + 2 trailing spaces.
const LONG_PAD: usize = 6;

fn to_arg_name(ident: &Ident) -> String {
    let mut name = ident.to_string().replace('_', "-");
    name.make_ascii_lowercase();

    name
}

fn to_help(arg: ArgView, max_width: usize) -> String {
    let name = to_arg_name(arg.name);
    let ty = arg.ty_help.as_str();
    let pad = " ".repeat(max_width + LONG_PAD);
    let help = arg.doc.join(&format!("\n{pad}"));

    if let Some(ch) = arg.short {
        let width = max_width - SHORT_PAD - name.len();

        format!("  -{ch} --{name}{ty:<width$}  {help}\n")
    } else {
        format!("  --{name}{ty:<max_width$}  {help}\n")
    }
}

fn get_max_width<'a, I>(iter: I) -> usize
where
    I: Iterator<Item = ArgView<'a>>,
{
    iter.fold(0, |acc, view| {
        let short = view.short.map(|_| SHORT_PAD).unwrap_or_default();

        acc.max(view.name.to_string().len() + view.ty_help.as_str().len() + short)
    })
}

fn dedupe<'a>(dupes: &mut HashMap<char, &'a Ident>, arg: ArgView<'a>) -> syn::Result<()> {
    if let Some(ch) = arg.short {
        if let Some(other) = dupes.get(&ch) {
            let msg =
                format!("Only one short arg is allowed. `-{ch}` also used on field `{other}`");

            return Err(syn::parse::Error::new(arg.name.span(), msg));
        }

        dupes.insert(ch, arg.name);
    }

    Ok(())
}
