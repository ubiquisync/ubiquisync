use ubiquisync_core::{
    crypto::credentials::Credentials,
    event::{EventBus, Subscribe, event_bus},
    ids::{AppId, ContainerId},
    uuid::Uuid,
};
use ubiquisync_sql::{
    Exec, ExecError, SqlQueryStore,
    db::{Db, DbError, DbRow, DbValue},
    dialect::SqlDialect,
    replica::Replica,
};

use crate::{
    error::TablesError, op::Op, reducer::Reducer, schema::TableSchema, watch::ChangeEvent,
};

pub struct StoreImpl {
    replica: Replica<Reducer>,
    event_bus: EventBus<ChangeEvent>,
}

impl StoreImpl {
    pub async fn new(
        app_magic: AppId,
        container_id: ContainerId,
        credentials: Box<dyn Credentials>,
        prefix: &str,
        tables: &[TableSchema],
        db: Box<dyn Db>,
    ) -> Result<Self, TablesError> {
        let (event_handler, event_bus) = event_bus();
        let reducer =
            Reducer::new(container_id, prefix, tables, db.as_ref(), event_handler).await?;
        let replica = Replica::new(app_magic, db, reducer, credentials).await?;
        Ok(Self { replica, event_bus })
    }
}

#[async_trait::async_trait]
impl Exec<Op> for StoreImpl {
    async fn exec(&self, server_user_id: Option<Uuid>, op: Op) -> Result<(), ExecError> {
        self.replica.exec(server_user_id, op).await
    }
}

#[async_trait::async_trait]
impl SqlQueryStore for StoreImpl {
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        self.replica.query(sql, params).await
    }

    fn dialect(&self) -> SqlDialect {
        self.replica.dialect()
    }
}

impl Subscribe<ChangeEvent> for StoreImpl {
    fn subscribe(
        &self,
        target: <ChangeEvent as ubiquisync_core::event::RoutableEvent>::Target,
    ) -> ubiquisync_core::event::Subscription<ChangeEvent> {
        self.event_bus.subscribe(target)
    }
}
