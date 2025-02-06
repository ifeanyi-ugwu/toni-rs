use proc_macro2::TokenStream;
use quote::quote;
use syn::visit_mut::{self, VisitMut};
use syn::{Block, Expr, Ident, Local, Member, Pat, PatType, Type, parse_quote};

use super::create_struct_name::create_field_struct_name;
struct ExprModifier {
    provider_names: Vec<(Ident, Ident)>,
    modified_exprs: Vec<(Ident, Ident, Ident)>,
    ty: Option<Type>,
    self_name: Ident,
}

impl ExprModifier {
    fn new(provider_names: Vec<(Ident, Ident)>, self_name: Ident) -> Self {
        Self {
            provider_names,
            modified_exprs: Vec::new(),
            ty: None,
            self_name,
        }
    }

    fn get_modified_exprs(self) -> Vec<(Ident, Ident, Ident)> {
        self.modified_exprs
    }

    fn put_box_in_expr(&self, exprs: syn::punctuated::Iter<'_, Expr>) -> Vec<TokenStream> {
        exprs
            .map(|expr| {
                quote! {
                    Box::new(#expr)
                }
            })
            .collect()
    }

    fn put_inject_type(&mut self, ty: Option<Type>) {
        self.ty = ty;
    }
}

impl VisitMut for ExprModifier {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if let Expr::MethodCall(method_call) = &expr {
            if let Expr::Field(expr_field) = &*method_call.receiver {
                if let Member::Named(ident) = &expr_field.member {
                    let ident_clone = ident.clone();
                    let method_args_clone = &method_call.args.clone();
                    let method_call_name = &method_call.method.clone();
                    for provide_name in &self.provider_names {
                        if ident_clone == provide_name.0 {
                            let method_name = method_call_name;
                            let new_field_name =
                                create_field_struct_name(&provide_name.1.to_string(), &method_name);
                            let args = self.put_box_in_expr(method_args_clone.iter());
                            self.modified_exprs.push((
                                provide_name.1.clone(),
                                method_name.clone(),
                                new_field_name.clone(),
                            ));
                            let type_inject = match &self.ty {
                                Some(ty) => ty,
                                None => panic!("Precisa de tipo"),
                            };
                            let new_expr: Expr = parse_quote! {
                                *self.#new_field_name.execute(vec![#(#args),*]).downcast::<#type_inject>().unwrap()
                            };
                            *expr = new_expr;
                        }
                    }
                }
            } else if let Expr::Path(expr_path) = &*method_call.receiver {
                if let Some(segment) = expr_path.path.segments.last() {
                    if segment.ident == "self" {
                        let method_args_clone = method_call.args.clone();
                        let method_name = method_call.method.clone();
                        let new_method_name =
                            create_field_struct_name(&self.self_name.to_string(), &method_name);
                        let args = self.put_box_in_expr(method_args_clone.iter());
                        self.modified_exprs.push((
                            self.self_name.clone(),
                            method_name.clone(),
                            new_method_name.clone(),
                        ));
                        let type_inject = match &self.ty {
                            Some(ty) => ty,
                            None => panic!("Precisa de tipo"),
                        };
                        let new_expr: Expr = parse_quote! {
                            *self.#new_method_name.execute(vec![#(#args),*]).downcast::<#type_inject>().unwrap()
                        };
                        *expr = new_expr;
                    }
                }
            }
        }

        visit_mut::visit_expr_mut(self, expr);
    }
}

pub fn modify_method_body(
    method_body: &mut Block,
    provider_names: Vec<(Ident, Ident)>,
    self_name: Ident,
) -> Vec<(Ident, Ident, Ident)> {
    let mut modifier = ExprModifier::new(provider_names, self_name);
    for stmt in &mut method_body.stmts {
        if let syn::Stmt::Expr(expr, _) = stmt {
            modifier.visit_expr_mut(expr);
        }
        if let syn::Stmt::Local(Local {
            init: Some(init),
            pat,
            ..
        }) = stmt
        {
            if let Pat::Type(PatType { ty, .. }) = pat.clone() {
                let type_inject = Some(*ty);
                modifier.put_inject_type(type_inject);
            }
            modifier.visit_expr_mut(&mut init.expr);
        }
    }
    modifier.get_modified_exprs()
}
