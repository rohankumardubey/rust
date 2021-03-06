use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::visitors::LocalUsedVisitor;
use clippy_utils::{higher, is_lang_ctor, path_to_local, peel_ref_operators, SpanlessEq};
use if_chain::if_chain;
use rustc_hir::LangItem::OptionNone;
use rustc_hir::{Expr, ExprKind, Guard, HirId, Pat, PatKind, StmtKind};
use rustc_lint::{LateContext, LateLintPass};
use rustc_session::{declare_lint_pass, declare_tool_lint};
use rustc_span::{MultiSpan, Span};

declare_clippy_lint! {
    /// ### What it does
    /// Finds nested `match` or `if let` expressions where the patterns may be "collapsed" together
    /// without adding any branches.
    ///
    /// Note that this lint is not intended to find _all_ cases where nested match patterns can be merged, but only
    /// cases where merging would most likely make the code more readable.
    ///
    /// ### Why is this bad?
    /// It is unnecessarily verbose and complex.
    ///
    /// ### Example
    /// ```rust
    /// fn func(opt: Option<Result<u64, String>>) {
    ///     let n = match opt {
    ///         Some(n) => match n {
    ///             Ok(n) => n,
    ///             _ => return,
    ///         }
    ///         None => return,
    ///     };
    /// }
    /// ```
    /// Use instead:
    /// ```rust
    /// fn func(opt: Option<Result<u64, String>>) {
    ///     let n = match opt {
    ///         Some(Ok(n)) => n,
    ///         _ => return,
    ///     };
    /// }
    /// ```
    pub COLLAPSIBLE_MATCH,
    style,
    "Nested `match` or `if let` expressions where the patterns may be \"collapsed\" together."
}

declare_lint_pass!(CollapsibleMatch => [COLLAPSIBLE_MATCH]);

impl<'tcx> LateLintPass<'tcx> for CollapsibleMatch {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &Expr<'tcx>) {
        if let Some(higher::IfLet {
            let_pat,
            if_then,
            if_else,
            ..
        }) = higher::IfLet::hir(expr)
        {
            check_arm(cx, if_then, None, let_pat, if_else);

            check_if_let(cx, if_then, let_pat);
        }

        if let ExprKind::Match(_expr, arms, _source) = expr.kind {
            if let Some(wild_arm) = arms.iter().rfind(|arm| is_wild_like(cx, &arm.pat.kind, &arm.guard)) {
                for arm in arms {
                    check_arm(cx, arm.body, arm.guard.as_ref(), arm.pat, Some(wild_arm.body));
                }
            }

            if let Some(first_arm) = arms.get(0) {
                check_if_let(cx, &first_arm.body, &first_arm.pat);
            }
        }
    }
}

fn check_arm<'tcx>(
    cx: &LateContext<'tcx>,
    outer_block: &'tcx Expr<'tcx>,
    outer_guard: Option<&Guard<'tcx>>,
    outer_pat: &'tcx Pat<'tcx>,
    wild_outer_block: Option<&'tcx Expr<'tcx>>,
) {
    let expr = strip_singleton_blocks(outer_block);
    if_chain! {
        if let ExprKind::Match(expr_in, arms_inner, _) = expr.kind;
        // the outer arm pattern and the inner match
        if expr_in.span.ctxt() == outer_pat.span.ctxt();
        // there must be no more than two arms in the inner match for this lint
        if arms_inner.len() == 2;
        // no if guards on the inner match
        if arms_inner.iter().all(|arm| arm.guard.is_none());
        // match expression must be a local binding
        // match <local> { .. }
        if let Some(binding_id) = path_to_local(peel_ref_operators(cx, expr_in));
        // one of the branches must be "wild-like"
        if let Some(wild_inner_arm_idx) = arms_inner.iter().rposition(|arm_inner| is_wild_like(cx, &arm_inner.pat.kind, &arm_inner.guard));
        let (wild_inner_arm, non_wild_inner_arm) =
            (&arms_inner[wild_inner_arm_idx], &arms_inner[1 - wild_inner_arm_idx]);
        if !pat_contains_or(non_wild_inner_arm.pat);
        // the binding must come from the pattern of the containing match arm
        // ..<local>.. => match <local> { .. }
        if let Some(binding_span) = find_pat_binding(outer_pat, binding_id);
        // the "wild-like" branches must be equal
        if wild_outer_block.map(|el| SpanlessEq::new(cx).eq_expr(wild_inner_arm.body, el)).unwrap_or(true);
        // the binding must not be used in the if guard
        let mut used_visitor = LocalUsedVisitor::new(cx, binding_id);
        if match outer_guard {
            None => true,
            Some(Guard::If(expr) | Guard::IfLet(_, expr)) => !used_visitor.check_expr(expr),
        };
        // ...or anywhere in the inner match
        if !arms_inner.iter().any(|arm| used_visitor.check_arm(arm));
        then {
            span_lint_and_then(
                cx,
                COLLAPSIBLE_MATCH,
                expr.span,
                "unnecessary nested match",
                |diag| {
                    let mut help_span = MultiSpan::from_spans(vec![binding_span, non_wild_inner_arm.pat.span]);
                    help_span.push_span_label(binding_span, "replace this binding".into());
                    help_span.push_span_label(non_wild_inner_arm.pat.span, "with this pattern".into());
                    diag.span_help(help_span, "the outer pattern can be modified to include the inner pattern");
                },
            );
        }
    }
}

fn check_if_let<'tcx>(cx: &LateContext<'tcx>, outer_expr: &'tcx Expr<'tcx>, outer_pat: &'tcx Pat<'tcx>) {
    let block_inner = strip_singleton_blocks(outer_expr);
    if_chain! {
        if let Some(higher::IfLet { if_then: inner_if_then, let_expr: inner_let_expr, let_pat: inner_let_pat, .. }) = higher::IfLet::hir(block_inner);
        if let Some(binding_id) = path_to_local(peel_ref_operators(cx, inner_let_expr));
        if let Some(binding_span) = find_pat_binding(outer_pat, binding_id);
        let mut used_visitor = LocalUsedVisitor::new(cx, binding_id);
        if !used_visitor.check_expr(inner_if_then);
        then {
            span_lint_and_then(
                cx,
                COLLAPSIBLE_MATCH,
                block_inner.span,
                "unnecessary nested `if let` or `match`",
                |diag| {
                    let mut help_span = MultiSpan::from_spans(vec![binding_span, inner_let_pat.span]);
                    help_span.push_span_label(binding_span, "replace this binding".into());
                    help_span.push_span_label(inner_let_pat.span, "with this pattern".into());
                    diag.span_help(help_span, "the outer pattern can be modified to include the inner pattern");
                },
            );
        }
    }
}

fn strip_singleton_blocks<'hir>(mut expr: &'hir Expr<'hir>) -> &'hir Expr<'hir> {
    while let ExprKind::Block(block, _) = expr.kind {
        match (block.stmts, block.expr) {
            ([stmt], None) => match stmt.kind {
                StmtKind::Expr(e) | StmtKind::Semi(e) => expr = e,
                _ => break,
            },
            ([], Some(e)) => expr = e,
            _ => break,
        }
    }
    expr
}

/// A "wild-like" pattern is wild ("_") or `None`.
/// For this lint to apply, both the outer and inner patterns
/// must have "wild-like" branches that can be combined.
fn is_wild_like(cx: &LateContext<'_>, pat_kind: &PatKind<'_>, arm_guard: &Option<Guard<'_>>) -> bool {
    if arm_guard.is_some() {
        return false;
    }
    match pat_kind {
        PatKind::Binding(..) | PatKind::Wild => true,
        PatKind::Path(ref qpath) => is_lang_ctor(cx, qpath, OptionNone),
        _ => false,
    }
}

fn find_pat_binding(pat: &Pat<'_>, hir_id: HirId) -> Option<Span> {
    let mut span = None;
    pat.walk_short(|p| match &p.kind {
        // ignore OR patterns
        PatKind::Or(_) => false,
        PatKind::Binding(_bm, _, _ident, _) => {
            let found = p.hir_id == hir_id;
            if found {
                span = Some(p.span);
            }
            !found
        },
        _ => true,
    });
    span
}

fn pat_contains_or(pat: &Pat<'_>) -> bool {
    let mut result = false;
    pat.walk(|p| {
        let is_or = matches!(p.kind, PatKind::Or(_));
        result |= is_or;
        !is_or
    });
    result
}
