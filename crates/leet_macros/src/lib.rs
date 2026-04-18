//! LEET Macros - Procedural macros for the LEET engine
//!
//! Provides the #[leet_main] attribute macro for managed mode entry points.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

/// Marks a function as the entry point for a LEET game in managed mode.
///
/// This macro generates a `main()` function that sets up the engine
/// and calls your game setup function.
///
/// # Example
///
/// ```ignore
/// use leet::prelude::*;
///
/// #[leet_main]
/// fn game_setup(app: &mut App) {
///     app.add_system(MySystem);
/// }
/// ```
#[proc_macro_attribute]
pub fn leet_main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_vis = &input_fn.vis;

    // Generate the expanded code
    let expanded = quote! {
        // Keep the original function for potential reuse
        #fn_vis fn #fn_name(#fn_inputs) #fn_block

        // Generate the main function
        fn main() {
            let mut app = App::new();

            // Call the user's setup function
            #fn_name(&mut app);


            // Run the application
            app.run();
        }
    };

    TokenStream::from(expanded)
}
