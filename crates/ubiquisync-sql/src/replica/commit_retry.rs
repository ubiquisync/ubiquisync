impl<R: Reducer> ReplicaInner<R> {
    pub(crate) async fn retry_commit(&self) {
        // TODO
        // 1. select any streams where commit_size < head_size AND commit_error == NULL
        // 2. select any streams where commit_error is HLC and clock has advanced
        // 3. other conditions: waiting for peer, key or software upgrade
    }
}
