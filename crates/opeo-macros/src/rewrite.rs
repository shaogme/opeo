use syn::{
    Block, Expr, ExprAsync, ExprBlock, ExprBreak, ExprCall, ExprClosure, ExprForLoop, ExprLoop,
    ExprMacro, ExprPath, ExprReturn, ExprTryBlock, ExprWhile, Ident, Item, Path, Stmt, StmtMacro,
    Type, parse_quote,
    visit_mut::{self, VisitMut},
};

pub(super) struct RewriteTry {
    runtime_path: Path,
    source_result_type: Type,
    rewrite_question_marks: bool,
}

impl VisitMut for RewriteTry {
    fn visit_expr_mut(&mut self, expression: &mut Expr) {
        if let Expr::Try(try_expression) = expression
            && self.rewrite_question_marks
        {
            self.visit_expr_mut(&mut try_expression.expr);
            let inner = &try_expression.expr;
            let runtime_path = &self.runtime_path;
            *expression = parse_quote!(#runtime_path::__opeo_try_value!(out, #inner));
            return;
        }
        visit_mut::visit_expr_mut(self, expression);
    }

    fn visit_expr_closure_mut(&mut self, _expression: &mut ExprClosure) {}

    fn visit_expr_async_mut(&mut self, _expression: &mut ExprAsync) {}

    fn visit_expr_try_block_mut(&mut self, expression: &mut ExprTryBlock) {
        let rewrite_question_marks = self.rewrite_question_marks;
        self.rewrite_question_marks = false;
        self.visit_block_mut(&mut expression.block);
        self.rewrite_question_marks = rewrite_question_marks;
    }

    fn visit_expr_return_mut(&mut self, expression: &mut ExprReturn) {
        if let Some(value) = &mut expression.expr {
            self.rewrite_result_tail(value);
            self.visit_expr_mut(value);
        }
    }

    fn visit_item_mut(&mut self, _item: &mut Item) {}
}

impl RewriteTry {
    pub(super) fn new(runtime_path: Path, source_result_type: Type) -> Self {
        Self {
            runtime_path,
            source_result_type,
            rewrite_question_marks: true,
        }
    }

    pub(super) fn visit_function_body(&mut self, block: &mut Block) {
        self.rewrite_block_tail(block);
        self.visit_block_mut(block);
    }

    fn rewrite_block_tail(&mut self, block: &mut Block) {
        if let Some(Stmt::Expr(expression, _)) = block.stmts.last_mut() {
            self.rewrite_result_tail(expression);
        }
    }

    fn rewrite_result_tail(&mut self, expression: &mut Expr) {
        match expression {
            Expr::Call(call)
                if is_result_variant_call(call, "Err") || is_result_variant_call(call, "Ok") =>
            {
                self.rewrite_result_call(expression);
            }
            Expr::Macro(macro_expression)
                if macro_expression
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "__opeo_try_value") =>
            {
                self.rewrite_result_expression(expression);
            }
            Expr::Macro(macro_expression)
                if macro_expression
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "opeo_try") =>
            {
                let runtime_path = &self.runtime_path;
                let value = expression.clone();
                *expression = parse_quote!(#runtime_path::OResult::success(#value));
            }
            Expr::Macro(macro_expression) if is_diverging_macro(macro_expression) => {}
            Expr::Block(block) => self.rewrite_block_tail(&mut block.block),
            Expr::If(if_expression) => {
                self.rewrite_block_tail(&mut if_expression.then_branch);
                if let Some((_, else_expression)) = &mut if_expression.else_branch {
                    self.rewrite_result_tail(else_expression);
                }
            }
            Expr::Match(match_expression) => {
                for arm in &mut match_expression.arms {
                    self.rewrite_result_tail(&mut arm.body);
                }
            }
            Expr::Paren(paren) => self.rewrite_result_tail(&mut paren.expr),
            Expr::Group(group) => self.rewrite_result_tail(&mut group.expr),
            Expr::Unsafe(unsafe_expression) => {
                self.rewrite_block_tail(&mut unsafe_expression.block);
            }
            Expr::Loop(loop_expression) if !loop_has_exit(loop_expression) => {}
            Expr::Break(break_expression) => {
                if let Some(value) = &mut break_expression.expr {
                    self.rewrite_result_tail(value);
                }
            }
            Expr::Continue(_) => {}
            Expr::Return(_) => {}
            _ => self.rewrite_result_expression(expression),
        }
    }

    fn rewrite_result_expression(&self, expression: &mut Expr) {
        let runtime_path = &self.runtime_path;
        let source_result_type = &self.source_result_type;
        let result = expression.clone();
        *expression = parse_quote!({
            let __opeo_result: #source_result_type = #result;
            #runtime_path::__OpeoTry::__opeo_try(
                __opeo_result,
                out.reborrow(),
            )
        });
    }

    fn rewrite_result_call(&mut self, expression: &mut Expr) {
        let Expr::Call(call) = expression else {
            return;
        };
        if call.args.len() != 1 {
            return;
        }

        let is_error = is_result_variant_call(call, "Err");
        let Some(value) = call.args.pop() else {
            return;
        };
        let runtime_path = &self.runtime_path;
        if is_error {
            *expression = parse_quote!(#runtime_path::OResult::failed(
                out.reborrow().fail(#value)
            ));
        } else {
            *expression = parse_quote!(#runtime_path::OResult::success(#value));
        }
    }
}

fn loop_has_exit(loop_expression: &ExprLoop) -> bool {
    let target_label = loop_expression
        .label
        .as_ref()
        .map(|label| label.name.ident.clone());
    let mut visitor = LoopExitVisitor {
        target_label,
        nested_loop_depth: 0,
        nested_labels: Vec::new(),
        found: false,
    };
    let mut body = loop_expression.body.clone();
    visitor.visit_block_mut(&mut body);
    visitor.found
}

struct LoopExitVisitor {
    target_label: Option<Ident>,
    nested_loop_depth: usize,
    nested_labels: Vec<Ident>,
    found: bool,
}

impl VisitMut for LoopExitVisitor {
    fn visit_expr_break_mut(&mut self, expression: &mut ExprBreak) {
        let exits_target = match &expression.label {
            Some(label) => {
                self.target_label.as_ref() == Some(&label.ident)
                    && !self.nested_labels.contains(&label.ident)
            }
            None => self.nested_loop_depth == 0,
        };
        self.found |= exits_target;
        if let Some(value) = &mut expression.expr {
            self.visit_expr_mut(value);
        }
    }

    fn visit_expr_loop_mut(&mut self, expression: &mut ExprLoop) {
        self.visit_nested_loop(expression.label.as_ref().map(|label| &label.name.ident));
        visit_mut::visit_expr_loop_mut(self, expression);
        self.nested_loop_depth -= 1;
        if expression.label.is_some() {
            self.nested_labels.pop();
        }
    }

    fn visit_expr_for_loop_mut(&mut self, expression: &mut ExprForLoop) {
        self.visit_nested_loop(expression.label.as_ref().map(|label| &label.name.ident));
        visit_mut::visit_expr_for_loop_mut(self, expression);
        self.nested_loop_depth -= 1;
        if expression.label.is_some() {
            self.nested_labels.pop();
        }
    }

    fn visit_expr_while_mut(&mut self, expression: &mut ExprWhile) {
        self.visit_nested_loop(expression.label.as_ref().map(|label| &label.name.ident));
        visit_mut::visit_expr_while_mut(self, expression);
        self.nested_loop_depth -= 1;
        if expression.label.is_some() {
            self.nested_labels.pop();
        }
    }

    fn visit_expr_block_mut(&mut self, expression: &mut ExprBlock) {
        if let Some(label) = &expression.label {
            self.nested_labels.push(label.name.ident.clone());
            visit_mut::visit_expr_block_mut(self, expression);
            self.nested_labels.pop();
        } else {
            visit_mut::visit_expr_block_mut(self, expression);
        }
    }

    fn visit_expr_closure_mut(&mut self, _expression: &mut ExprClosure) {}

    fn visit_expr_async_mut(&mut self, _expression: &mut ExprAsync) {}

    fn visit_expr_macro_mut(&mut self, _expression: &mut ExprMacro) {
        self.found = true;
    }

    fn visit_stmt_macro_mut(&mut self, _statement: &mut StmtMacro) {
        self.found = true;
    }

    fn visit_item_mut(&mut self, _item: &mut Item) {}
}

impl LoopExitVisitor {
    fn visit_nested_loop(&mut self, label: Option<&Ident>) {
        self.nested_loop_depth += 1;
        if let Some(label) = label {
            self.nested_labels.push(label.clone());
        }
    }
}

fn is_diverging_macro(expression: &ExprMacro) -> bool {
    expression.mac.path.segments.last().is_some_and(|segment| {
        matches!(
            segment.ident.to_string().as_str(),
            "panic" | "unreachable" | "todo" | "unimplemented"
        )
    })
}

fn is_result_variant_call(call: &ExprCall, variant: &str) -> bool {
    let Expr::Path(ExprPath {
        qself: None, path, ..
    }) = call.func.as_ref()
    else {
        return false;
    };
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == variant)
}

#[cfg(test)]
mod tests {
    use super::{RewriteTry, is_diverging_macro, loop_has_exit};
    use quote::quote;
    use syn::{Expr, ExprLoop, ExprMacro, parse_quote};

    #[test]
    fn loop_exit_detection_only_counts_breaks_that_exit_target() {
        let loop_with_inner_break: ExprLoop = parse_quote!(loop {
            while true {
                break;
            }
        });
        let loop_with_exit: ExprLoop = parse_quote!(loop {
            if should_stop {
                break Ok::<u32, ParseError>(1);
            }
        });
        let loop_with_macro_exit: ExprLoop = parse_quote!(loop {
            if should_stop {
                break_loop_with_result!(Ok::<u32, ParseError>(1));
            }
        });
        let labeled_loop_with_exit: ExprLoop = parse_quote!('outer: loop {
            loop {
                break 'outer Ok(1);
            }
        });

        assert!(!loop_has_exit(&loop_with_inner_break));
        assert!(loop_has_exit(&loop_with_exit));
        assert!(loop_has_exit(&loop_with_macro_exit));
        assert!(loop_has_exit(&labeled_loop_with_exit));
    }
    #[test]
    fn tail_rewriting_recurses_through_parentheses_and_leaves_diverging_macros_alone() {
        let mut rewrite =
            RewriteTry::new(parse_quote!(::opeo), parse_quote!(Result<u32, ParseError>));
        let mut parenthesized_tail: Expr = parse_quote!((Ok(7)));
        rewrite.rewrite_result_tail(&mut parenthesized_tail);
        let rewritten = quote!(#parenthesized_tail).to_string();
        assert!(rewritten.contains("OResult :: success (7)"));

        let panic_macro: ExprMacro = parse_quote!(panic!("stop"));
        let custom_macro: ExprMacro = parse_quote!(custom::never_returns!());
        assert!(is_diverging_macro(&panic_macro));
        assert!(!is_diverging_macro(&custom_macro));
    }
}
