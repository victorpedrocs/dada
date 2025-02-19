//! The "bir" (pronounced "beer") is the "base ir" that we use
//! for interpretation.

use std::collections::BTreeSet;

use crate::{
    class::Class,
    code::validated::op::Op,
    function::Function,
    in_ir_db::InIrDb,
    input_file::InputFile,
    intrinsic::Intrinsic,
    origin_table::HasOriginIn,
    prelude::InIrDbExt,
    span::{Anchored, FileSpan, Span},
    storage::Atomic,
    word::Word,
};
use dada_id::{id, prelude::*, tables};
use salsa::DebugWithDb;

use super::{syntax, validated};

#[salsa::tracked]
pub struct Bir {
    /// Name of file containing the code from which this Bir was created.
    input_file: InputFile,

    /// Name of function containing the code from which this Bir was created.
    function_name: Word,

    /// Syntax tree from which this Bir was created.
    syntax_tree: syntax::Tree,

    /// The BIR bir
    #[return_ref]
    data: BirData,

    /// Origins of expr in the BIR. Used to trace back to a source span.
    #[return_ref]
    origins: Origins,
}

impl Anchored for Bir {
    fn input_file(&self, db: &dyn crate::Db) -> InputFile {
        Bir::input_file(*self, db)
    }
}

impl DebugWithDb<dyn crate::Db + '_> for Bir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn crate::Db) -> std::fmt::Result {
        let self_in_ir_db = &self.in_ir_db(db.as_dyn_ir_db());
        DebugWithDb::fmt(self.data(db), f, self_in_ir_db)
    }
}

impl InIrDb<'_, Bir> {
    fn tables(&self) -> &Tables {
        &self.data(self.db()).tables
    }
}

impl Bir {
    /// Given a `syntax_node` within this BIR, find its span. This operation
    /// is to be avoided unless reporting a diagnostic or really needed, because
    /// it induces a dependency on the *precise span* of the expression and hence
    /// will require re-execution if most anything in the source file changes, even
    /// just adding whitespace.
    pub fn span_of(
        self,
        db: &dyn crate::Db,
        syntax_node: impl HasOriginIn<syntax::Spans, Origin = Span>,
    ) -> FileSpan {
        let syntax_tree = self.syntax_tree(db);
        syntax_tree.spans(db)[syntax_node].anchor_to(db, self)
    }
}

/// Stores the ast for a function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BirData {
    /// Interning tables for expressions and the like.
    pub tables: Tables,

    /// First N local variables are the parameters.
    pub num_parameters: usize,

    /// The starting point in the control flow
    pub start_point: ControlPoint,
}

impl DebugWithDb<InIrDb<'_, Bir>> for BirData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("bir::Bir");
        dbg.field("start_point", &self.start_point);

        for cp in self.control_points() {
            dbg.field(&format!("{cp:?}"), &cp.data(&self.tables).debug(db));
        }

        dbg.finish()
    }
}

impl BirData {
    pub fn new(tables: Tables, num_parameters: usize, start_point: ControlPoint) -> Self {
        Self {
            tables,
            num_parameters,
            start_point,
        }
    }

    pub fn tables(&self) -> &Tables {
        &self.tables
    }

    pub fn num_parameters(&self) -> usize {
        self.num_parameters
    }

    pub fn parameters(&self) -> impl Iterator<Item = LocalVariable> {
        LocalVariable::range(0, self.num_parameters)
    }

    pub fn max_local_variable(&self) -> LocalVariable {
        LocalVariable::max_key(&self.tables)
    }

    pub fn control_points(&self) -> BTreeSet<ControlPoint> {
        let mut points = BTreeSet::new();
        let mut stack = vec![self.start_point];

        while let Some(p) = stack.pop() {
            if points.insert(p) {
                stack.extend(p.successors(self));
            }
        }

        points
    }
}

tables! {
    /// Tables that store the bir for expr in the AST.
    /// You can use `tables[expr]` (etc) to access the bir.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Tables {
        local_variables: alloc LocalVariable => LocalVariableData,
        control_points: alloc ControlPoint => ControlPointData,
        exprs: alloc Expr => ExprData,
        places: alloc Place => PlaceData,
        target_places: alloc TargetPlace => TargetPlaceData,
        name: alloc Name => NameData,
    }
}

origin_table! {
    /// Side table that contains the spans for everything in a syntax tree.
    /// This isn't normally needed except for diagnostics, so it's
    /// kept separate to avoid reducing incremental reuse.
    /// You can request it by invoking the `spans`
    /// method in the `dada_parse` prelude.
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Origins {
        local_variables: LocalVariable => validated::LocalVariableOrigin,
        control_points: ControlPoint => syntax::Expr,
        expr: Expr => syntax::Expr,
        place: Place => syntax::Expr,
        target_place: TargetPlace => syntax::Expr,
        name: Name => syntax::Name,
    }
}

id!(
    /// A *control point* is a point in the control-flow graph;
    /// it can be either a [`Statement`][] (which has a single successor)
    /// or a [`Terminator`][] (which has a custom set of successors).
    pub struct ControlPoint
);

impl DebugWithDb<InIrDb<'_, Bir>> for ControlPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, _db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        write!(f, "ControlPoint({})", u32::from(*self))
    }
}

impl ControlPoint {
    pub fn successors(&self, bir_data: &BirData) -> Vec<ControlPoint> {
        match self.data(&bir_data.tables) {
            ControlPointData::Statement(s) => vec![s.next],
            ControlPointData::Terminator(t) => t.successors(),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum ControlPointData {
    Statement(StatementData),
    Terminator(TerminatorData),
}

impl DebugWithDb<InIrDb<'_, Bir>> for ControlPointData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            ControlPointData::Statement(s) => s.fmt(f, db),
            ControlPointData::Terminator(t) => t.fmt(f, db),
        }
    }
}

id!(pub struct LocalVariable);

impl DebugWithDb<InIrDb<'_, Bir>> for LocalVariable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        let id = u32::from(*self);
        let bir = self.data(db.tables());
        let name = bir.name.map(|n| n.as_str(db.db())).unwrap_or("temp");
        write!(f, "{name}{{{id}}}")
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash, Debug)]
pub struct LocalVariableData {
    /// Name given to this variable by the user.
    /// If it is None, then this is a temporary
    /// introduced by the compiler.
    pub name: Option<Word>,

    pub atomic: Atomic,
}

/// A *statement* is a node in the control-flow graph that performs an action
/// and which always has exactly 1 successor; statements can either start basic blocks
/// or be in the mid-points.
#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub struct StatementData {
    /// The action to be performed at this statement.
    pub action: ActionData,

    /// Next point in the control flow. During the "brewing" process,
    /// this is initially set to a "dummy" terminator which is later overwritten
    /// with the next statemenr or terminator.
    pub next: ControlPoint,
}

impl DebugWithDb<InIrDb<'_, Bir>> for StatementData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        let StatementData { action, next } = self;
        f.debug_tuple("Statement")
            .field(&action.debug(db))
            .field(&next.debug(db))
            .finish()
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum ActionData {
    /// No action. Created during construction.
    Noop,

    /// Assign the result of evaluating an expression to a place.
    /// This is the preferred form of assignment, and covers
    /// cases like `a := b` as well as `a := 22`. In these case, either
    /// (a) we know statically the declared mode for `a` and so we
    /// can prepare an expression like `b.give` or `b.lease` in advance
    /// or (b) the rhs is an rvalue, like `22`, and so is always given.
    AssignExpr(TargetPlace, Expr),

    /// Clears the value from the given local variable.
    Clear(LocalVariable),

    /// In terms of the semantics, this is a no-op.
    /// It is used by the time traveling debugger.
    ///
    /// It indicates the moment when one of the breakpoint expressions
    /// in the given file (identified by the usize index) is about
    /// to start executon.
    BreakpointStart(InputFile, usize),

    /// In terms of the semantics, this is a no-op.
    /// It is used by the time traveling debugger.
    ///
    /// It indicates the moment when one of the breakpoint expressions
    /// in the given file (identified by the usize index) is about
    /// to complete and produce the (optional) `Place` as its value.
    ///
    /// The `syntax::Expr` argument is the expression that just
    /// completed. This may not be the same as the expression on which
    /// the breakpoint was set, if that expression was part of a larger
    /// place or other "compound" that could not be executed independently.
    ///
    /// Any side-effects from the breakpoint will have taken place
    /// when this statement executes.
    BreakpointEnd(InputFile, usize, syntax::Expr, Option<Place>),
}

impl DebugWithDb<InIrDb<'_, Bir>> for ActionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            ActionData::Noop => f.debug_tuple("Noop").finish(),

            ActionData::AssignExpr(place, expr) => f
                .debug_tuple("AssignExpr")
                .field(&place.debug(db))
                .field(&expr.debug(db))
                .finish(),

            ActionData::Clear(lv) => f.debug_tuple("Clear").field(&lv.debug(db)).finish(),

            ActionData::BreakpointStart(input_file, index) => f
                .debug_tuple("BreakpoingStart")
                .field(&input_file.debug(db.db()))
                .field(index)
                .finish(),

            ActionData::BreakpointEnd(input_file, index, e, p) => f
                .debug_tuple("BreakpointEnd")
                .field(&input_file.debug(db.db()))
                .field(index)
                .field(e)
                .field(&p.debug(db))
                .finish(),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum TerminatorData {
    Goto(ControlPoint),
    If(Place, ControlPoint, ControlPoint),
    StartAtomic(ControlPoint),
    EndAtomic(ControlPoint),
    Return(Place),
    Assign(TargetPlace, TerminatorExpr, ControlPoint),
    Error,
    Panic,
}

impl TerminatorData {
    pub fn successors(&self) -> Vec<ControlPoint> {
        match *self {
            TerminatorData::Goto(c) => vec![c],
            TerminatorData::If(_, a, b) => vec![a, b],
            TerminatorData::StartAtomic(a) => vec![a],
            TerminatorData::EndAtomic(a) => vec![a],
            TerminatorData::Return(_) => vec![],
            TerminatorData::Assign(_, _, a) => vec![a],
            TerminatorData::Error => vec![],
            TerminatorData::Panic => vec![],
        }
    }
}

impl DebugWithDb<InIrDb<'_, Bir>> for TerminatorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            TerminatorData::Goto(block) => f.debug_tuple("Goto").field(block).finish(),
            TerminatorData::If(condition, if_true, if_false) => f
                .debug_tuple("If")
                .field(&condition.debug(db))
                .field(&if_true.debug(db))
                .field(&if_false.debug(db))
                .finish(),
            TerminatorData::StartAtomic(block) => {
                f.debug_tuple("StartAomic").field(&block.debug(db)).finish()
            }
            TerminatorData::EndAtomic(block) => {
                f.debug_tuple("EndAtomic").field(&block.debug(db)).finish()
            }
            TerminatorData::Return(value) => {
                f.debug_tuple("Return").field(&value.debug(db)).finish()
            }
            TerminatorData::Assign(target, expr, next) => f
                .debug_tuple("Assign")
                .field(&target.debug(db))
                .field(&expr.debug(db))
                .field(&next.debug(db))
                .finish(),
            TerminatorData::Error => f.debug_tuple("Error").finish(),
            TerminatorData::Panic => f.debug_tuple("Panic").finish(),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum TerminatorExpr {
    Await(Place),

    /// Call `function(arguments...)`. The `labels` for each
    /// argument are present as well.
    Call {
        function: Place,
        arguments: Vec<Place>,
        labels: Vec<Option<Name>>,
    },
}

impl DebugWithDb<InIrDb<'_, Bir>> for TerminatorExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            TerminatorExpr::Await(place) => f.debug_tuple("Await").field(&place.debug(db)).finish(),
            TerminatorExpr::Call {
                function,
                arguments,
                labels,
            } => f
                .debug_tuple("Call")
                .field(&function.debug(db))
                .field(&arguments.debug(db))
                .field(&labels.debug(db))
                .finish(),
        }
    }
}

id!(pub struct Expr);

impl DebugWithDb<InIrDb<'_, Bir>> for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        write!(f, "{:?}", self.data(db.tables()).debug(db))
    }
}
#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum ExprData {
    /// true, false
    BooleanLiteral(bool),

    /// `22i`, `22_222i`, etc
    SignedIntegerLiteral(i64),

    /// `22u`, `22_222u`, etc
    UnsignedIntegerLiteral(u64),

    /// `22`, `22_222`, etc
    IntegerLiteral(u64),

    /// `2.2`
    FloatLiteral(eq_float::F64),

    /// `"foo"` with no format strings
    StringLiteral(Word),

    /// `<value>.share`
    IntoShared(Place),

    /// `<place>.share`
    Share(Place),

    /// `expr.lease`
    Lease(Place),

    /// `expr.give`
    Give(Place),

    /// `()`
    Unit,

    /// `(a, b, ...)` (i.e., at least 2)
    Tuple(Vec<Place>),

    /// Concatenates a bunch of strings together from a format literal like
    /// `foo{bar}baz`
    Concatenate(Vec<Place>),

    /// `a + b`
    Op(Place, Op, Place),

    /// `- 1`
    Unary(Op, Place),

    /// parse or other error
    Error,
}

impl DebugWithDb<InIrDb<'_, Bir>> for ExprData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            ExprData::BooleanLiteral(b) => write!(f, "{b}"),
            ExprData::IntegerLiteral(w) => write!(f, "{w}"),
            ExprData::UnsignedIntegerLiteral(w) => write!(f, "{w}"),
            ExprData::SignedIntegerLiteral(w) => write!(f, "{w}"),
            ExprData::StringLiteral(w) => write!(f, "{:?}", w.as_str(db.db())),
            ExprData::FloatLiteral(w) => write!(f, "{w}"),
            ExprData::IntoShared(e) => write!(f, "{:?}.share", e.debug(db)),
            ExprData::Share(p) => write!(f, "{:?}.share", p.debug(db)),
            ExprData::Lease(p) => write!(f, "{:?}.lease", p.debug(db)),
            ExprData::Give(p) => write!(f, "{:?}.give", p.debug(db)),
            ExprData::Unit => write!(f, "()"),
            ExprData::Tuple(vars) => write_parenthesized_places(f, vars, db),
            ExprData::Concatenate(vars) => {
                write!(f, "Concatenate")?;
                write_parenthesized_places(f, vars, db)
            }
            ExprData::Op(lhs, op, rhs) => {
                write!(f, "{:?} {} {:?}", lhs.debug(db), op.str(), rhs.debug(db))
            }
            ExprData::Error => write!(f, "<error>"),
            ExprData::Unary(op, rhs) => {
                write!(f, "{} {:?}", op.str(), rhs.debug(db))
            }
        }
    }
}

fn write_parenthesized_places(
    f: &mut std::fmt::Formatter<'_>,
    vars: &[Place],
    db: &InIrDb<'_, Bir>,
) -> std::fmt::Result {
    write!(f, "(")?;
    for (v, i) in vars.iter().zip(0..) {
        if i > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{:?}", v.debug(db))?;
    }
    write!(f, ")")?;
    Ok(())
}

id!(pub struct Place);

impl DebugWithDb<InIrDb<'_, Bir>> for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        write!(f, "{:?}", self.data(db.tables()).debug(db))
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum PlaceData {
    LocalVariable(LocalVariable),
    Function(Function),
    Class(Class),
    Intrinsic(Intrinsic),
    Dot(Place, Word),
}

impl DebugWithDb<InIrDb<'_, Bir>> for PlaceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            PlaceData::LocalVariable(v) => write!(f, "{:?}", v.debug(db)),
            PlaceData::Function(func) => write!(f, "{:?}", func.debug(db.db())),
            PlaceData::Class(class) => write!(f, "{:?}", class.debug(db.db())),
            PlaceData::Intrinsic(intrinsic) => write!(f, "{intrinsic:?}"),
            PlaceData::Dot(p, id) => write!(f, "{:?}.{}", p.debug(db), id.as_str(db.db())),
        }
    }
}

id!(pub struct TargetPlace);

impl DebugWithDb<InIrDb<'_, Bir>> for TargetPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        write!(f, "{:?}", self.data(db.tables()).debug(db))
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum TargetPlaceData {
    LocalVariable(LocalVariable),
    Dot(Place, Word),
}

impl DebugWithDb<InIrDb<'_, Bir>> for TargetPlaceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        match self {
            TargetPlaceData::LocalVariable(v) => write!(f, "{:?}", v.debug(db)),
            TargetPlaceData::Dot(p, id) => write!(f, "{:?}.{}", p.debug(db), id.as_str(db.db())),
        }
    }
}

id!(pub struct Name);

impl DebugWithDb<InIrDb<'_, Bir>> for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &InIrDb<'_, Bir>) -> std::fmt::Result {
        write!(f, "{:?}", self.data(db.tables()).word.debug(db.db()))
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub struct NameData {
    pub word: Word,
}
