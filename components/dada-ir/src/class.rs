use crate::{
    code::syntax,
    input_file::InputFile,
    span::{Anchored, Span},
    word::Word,
};

#[salsa::tracked]
pub struct Class {
    #[id]
    name: Word,

    input_file: InputFile,

    #[return_ref]
    signature_syntax: syntax::Signature,

    /// Overall span of the class (including any body)
    span: Span,
}

impl Class {
    pub fn name_span(self, db: &dyn crate::Db) -> Span {
        let signature = self.signature_syntax(db);
        signature.spans[signature.name]
    }
}

impl Anchored for Class {
    fn input_file(&self, db: &dyn crate::Db) -> InputFile {
        Class::input_file(*self, db)
    }
}

impl<Db: ?Sized + crate::Db> salsa::DebugWithDb<Db> for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, _db: &Db) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
