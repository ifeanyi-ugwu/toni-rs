use quote::quote;
use syn::{Block, Expr, Stmt};

pub fn modify_return_method_body(method_body: &mut Block) {
  if let Some(last_stmt) = method_body.stmts.last_mut() {
      if let Stmt::Expr(expr, None) = last_stmt {
          let new_return_expr: Expr = syn::parse2(quote! {
              Box::new(#expr)
          })
          .unwrap();

          *expr = new_return_expr;
      } else {
          let new_return_expr: Expr = syn::parse2(quote! {
              Box::new(())
          })
          .unwrap();

          method_body.stmts.push(Stmt::Expr(new_return_expr, None));
      }
  }
}