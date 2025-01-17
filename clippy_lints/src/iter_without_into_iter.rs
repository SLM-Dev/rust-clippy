use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::get_parent_as_impl;
use clippy_utils::source::snippet;
use clippy_utils::ty::{implements_trait, make_normalized_projection};
use rustc_errors::Applicability;
use rustc_hir::{FnRetTy, ImplItemKind, ImplicitSelfKind, TyKind};
use rustc_lint::{LateContext, LateLintPass};
use rustc_session::{declare_lint_pass, declare_tool_lint};
use rustc_span::sym;

declare_clippy_lint! {
    /// ### What it does
    /// Looks for `iter` and `iter_mut` methods without an associated `IntoIterator for (&|&mut) Type` implementation.
    ///
    /// ### Why is this bad?
    /// It's not bad, but having them is idiomatic and allows the type to be used in for loops directly
    /// (`for val in &iter {}`), without having to first call `iter()` or `iter_mut()`.
    ///
    /// ### Example
    /// ```rust
    /// struct MySlice<'a>(&'a [u8]);
    /// impl<'a> MySlice<'a> {
    ///     pub fn iter(&self) -> std::slice::Iter<'a, u8> {
    ///         self.0.iter()
    ///     }
    /// }
    /// ```
    /// Use instead:
    /// ```rust
    /// struct MySlice<'a>(&'a [u8]);
    /// impl<'a> MySlice<'a> {
    ///     pub fn iter(&self) -> std::slice::Iter<'a, u8> {
    ///         self.0.iter()
    ///     }
    /// }
    /// impl<'a> IntoIterator for &MySlice<'a> {
    ///     type Item = &'a u8;
    ///     type IntoIter = std::slice::Iter<'a, u8>;
    ///     fn into_iter(self) -> Self::IntoIter {
    ///         self.iter()
    ///     }
    /// }
    /// ```
    #[clippy::version = "1.74.0"]
    pub ITER_WITHOUT_INTO_ITER,
    pedantic,
    "implementing `iter(_mut)` without an associated `IntoIterator for (&|&mut) Type` impl"
}
declare_lint_pass!(IterWithoutIntoIter => [ITER_WITHOUT_INTO_ITER]);

/// Checks if a given type is nameable in a trait (impl).
/// RPIT is stable, but impl Trait in traits is not (yet), so when we have
/// a function such as `fn iter(&self) -> impl IntoIterator`, we can't
/// suggest `type IntoIter = impl IntoIterator`.
fn is_nameable_in_impl_trait(ty: &rustc_hir::Ty<'_>) -> bool {
    !matches!(ty.kind, TyKind::OpaqueDef(..))
}

impl LateLintPass<'_> for IterWithoutIntoIter {
    fn check_impl_item(&mut self, cx: &LateContext<'_>, item: &rustc_hir::ImplItem<'_>) {
        let item_did = item.owner_id.to_def_id();
        let (borrow_prefix, expected_implicit_self) = match item.ident.name {
            sym::iter => ("&", ImplicitSelfKind::ImmRef),
            sym::iter_mut => ("&mut ", ImplicitSelfKind::MutRef),
            _ => return,
        };

        if let ImplItemKind::Fn(sig, _) = item.kind
            && let FnRetTy::Return(ret) = sig.decl.output
            && is_nameable_in_impl_trait(ret)
            && cx.tcx.generics_of(item_did).params.is_empty()
            && sig.decl.implicit_self == expected_implicit_self
            && sig.decl.inputs.len() == 1
            && let Some(imp) = get_parent_as_impl(cx.tcx, item.hir_id())
            && imp.of_trait.is_none()
            && let sig = cx.tcx.liberate_late_bound_regions(
                item_did,
                cx.tcx.fn_sig(item_did).instantiate_identity()
            )
            && let ref_ty = sig.inputs()[0]
            && let Some(into_iter_did) = cx.tcx.get_diagnostic_item(sym::IntoIterator)
            && let Some(iterator_did) = cx.tcx.get_diagnostic_item(sym::Iterator)
            && let ret_ty = sig.output()
            // Order is important here, we need to check that the `fn iter` return type actually implements `IntoIterator`
            // *before* normalizing `<_ as IntoIterator>::Item` (otherwise make_normalized_projection ICEs)
            && implements_trait(cx, ret_ty, iterator_did, &[])
            && let Some(iter_ty) = make_normalized_projection(
                cx.tcx,
                cx.param_env,
                iterator_did,
                sym!(Item),
                [ret_ty],
            )
            // Only lint if the `IntoIterator` impl doesn't actually exist
            && !implements_trait(cx, ref_ty, into_iter_did, &[])
        {
            let self_ty_snippet = format!("{borrow_prefix}{}", snippet(cx, imp.self_ty.span, ".."));

            span_lint_and_then(
                cx,
                ITER_WITHOUT_INTO_ITER,
                item.span,
                &format!("`{}` method without an `IntoIterator` impl for `{self_ty_snippet}`", item.ident),
                |diag| {
                    // Get the lower span of the `impl` block, and insert the suggestion right before it:
                    // impl X {
                    // ^   fn iter(&self) -> impl IntoIterator { ... }
                    // }
                    let span_behind_impl = cx.tcx
                        .def_span(cx.tcx.hir().parent_id(item.hir_id()).owner.def_id)
                        .shrink_to_lo();

                    let sugg = format!(
"
impl IntoIterator for {self_ty_snippet} {{
    type IntoIter = {ret_ty};
    type Iter = {iter_ty};
    fn into_iter() -> Self::IntoIter {{
        self.iter()
    }}
}}
"
                    );
                    diag.span_suggestion_verbose(
                        span_behind_impl,
                        format!("consider implementing `IntoIterator` for `{self_ty_snippet}`"),
                        sugg,
                        // Suggestion is on a best effort basis, might need some adjustments by the user
                        // such as adding some lifetimes in the associated types, or importing types.
                        Applicability::Unspecified,
                    );
            });
        }
    }
}
