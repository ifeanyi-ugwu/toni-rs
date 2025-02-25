use quote::quote;
use syn::{Block, Expr, Stmt, visit_mut::VisitMut};
struct ReturnBoxer;

impl VisitMut for ReturnBoxer {
    fn visit_expr_return_mut(&mut self, expr_return: &mut syn::ExprReturn) {
        let expr = expr_return
            .expr
            .take()
            .map(|e| syn::parse2::<Expr>(quote! { Box::new(#e) }).unwrap())
            .unwrap_or_else(|| syn::parse2::<Expr>(quote! { Box::new(()) }).unwrap());

        expr_return.expr = Some(Box::new(expr));

        syn::visit_mut::visit_expr_return_mut(self, expr_return);
    }
}

pub fn modify_return_method_body(method_body: &mut Block) {
    let mut visitor = ReturnBoxer;
    visitor.visit_block_mut(method_body);

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

    //if find return statement, replace it with Box::new(returned_expr)
    method_body.stmts.iter_mut().for_each(|stmt| {
        if let Stmt::Expr(Expr::Return(_), None) = stmt {
            println!("ReturnReturnReturn: {:?}", stmt);
            if let Stmt::Expr(Expr::Return(expr_return), None) = stmt {
                let returned_expr = &expr_return.expr;
                let new_return_expr: Expr = syn::parse2(quote! {
                    Box::new(#returned_expr)
                })
                .unwrap();
                println!("new_return_expr: {:?}", &expr_return);
                *expr_return = syn::ExprReturn {
                    attrs: expr_return.attrs.clone(),
                    return_token: expr_return.return_token,
                    expr: Some(Box::new(new_return_expr)),
                };
            }
        }
    });
}
