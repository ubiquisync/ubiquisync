pub struct Walker {}

#[derive(Debug, Clone, Copy)]
pub enum View {
    Effect,
    Prepare,
}

pub trait Walkable<Id> {
    type LogicalOp;
    type PhysicalOp;
    type PrepareOp;
    type LogicalOpErr;
    type EffectErr;
    type PrepareErr;

    fn logical_to_physical(
        &self,
        view: View,
        op: Self::LogicalOp,
    ) -> Result<Self::PhysicalOp, Self::LogicalOpErr>;

    fn apply(&mut self, id: Id, op: Self::PhysicalOp) -> Result<Self::PrepareOp, Self::EffectErr>;

    fn prepare_advance(&mut self, op: &Self::PrepareOp) -> Result<(), Self::PrepareErr>;
    fn prepare_retract(&mut self, op: &Self::PrepareOp) -> Result<(), Self::PrepareErr>;
}
