//! The syntax tree of the statements this slice accepts.

/// A literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    /// A string.
    Str(String),
    /// An integer.
    Int(i64),
    /// A decimal.
    Float(f64),
    /// A boolean.
    Bool(bool),
    /// The null literal.
    Null,
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `=`
    Eq,
    /// `<>` or `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

/// A text-search function call.
#[derive(Debug, Clone, PartialEq)]
pub struct Relevance {
    /// The function name, lower case.
    pub func: String,
    /// The fields searched and their boosts.
    pub fields: Vec<(String, f64)>,
    /// The query text.
    pub query: String,
    /// The named options, in the order written.
    pub options: Vec<(String, Lit)>,
}

/// A condition of a `WHERE`.
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    /// All of these.
    And(Vec<Pred>),
    /// Any of these.
    Or(Vec<Pred>),
    /// Not this.
    Not(Box<Pred>),
    /// A field against a literal.
    Cmp(String, CmpOp, Lit),
    /// A field between two literals, negated by the flag.
    Between(String, Lit, Lit, bool),
    /// A field among literals, negated by the flag.
    In(String, Vec<Lit>, bool),
    /// A field is (not, by the flag) null.
    IsNull(String, bool),
    /// A field like a pattern, negated by the flag.
    Like(String, String, bool),
    /// A text-search function.
    Rel(Relevance),
}

/// One item of a select list.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// `*`
    Star,
    /// Every field but these (PPL `fields - a, b`).
    StarExcept(Vec<String>),
    /// A field, with its alias.
    Field(String, Option<String>),
    /// An aggregate (`count`, `sum`, `avg`, `min`, `max`) over a field or `*`, with its alias.
    Agg(Agg, Option<String>),
    /// A scalar expression, its alias and its text as written (the column's name).
    Expr(crate::expr::Expr, Option<String>, String),
}

/// An aggregate call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agg {
    /// The function, lower case.
    pub func: String,
    /// The field, or `None` for `*`.
    pub field: Option<String>,
}

impl Agg {
    /// The name the plugin gives the column: `count(*)`, `sum(age)`.
    #[must_use]
    pub fn name(&self) -> String {
        // ALLOC: one name, bounded by the identifier limit.
        format!("{}({})", self.func, self.field.as_deref().unwrap_or("*"))
    }
}

/// A `HAVING` comparison of an aggregate (by call or alias) with a literal.
#[derive(Debug, Clone, PartialEq)]
pub struct Having {
    /// The aggregate's call or alias as written.
    pub target: String,
    /// The comparison.
    pub op: CmpOp,
    /// The literal.
    pub lit: Lit,
}

/// One `ORDER BY` key.
#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    /// The field.
    pub field: String,
    /// Whether descending.
    pub desc: bool,
    /// `NULLS FIRST` (true) or `NULLS LAST` (false) when written.
    pub nulls_first: Option<bool>,
}

/// The language a statement was written in: it decides type names and error texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// SQL.
    Sql,
    /// PPL.
    Ppl,
}

/// A `SELECT`.
#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub(crate) dialect: Dialect,
    pub(crate) items: Vec<Item>,
    pub(crate) from: Vec<String>,
    pub(crate) filter: Option<Pred>,
    pub(crate) distinct: bool,
    pub(crate) group: Vec<String>,
    pub(crate) having: Option<Having>,
    pub(crate) order: Vec<Order>,
    pub(crate) limit: Option<u64>,
    pub(crate) offset: u64,
}

impl Select {
    /// The names after `FROM`, as written (unquoted), before any resolution.
    #[must_use]
    pub fn index_names(&self) -> &[String] {
        &self.from
    }
}

/// A statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// A `SELECT`.
    Select(Box<Select>),
    /// `SHOW TABLES LIKE pattern`.
    ShowTables {
        /// The pattern.
        pattern: String,
    },
    /// `DESCRIBE TABLES LIKE pattern [COLUMNS LIKE pattern]`.
    DescribeTables {
        /// The table pattern.
        pattern: String,
        /// The column pattern.
        columns: Option<String>,
    },
}
