use crate::code_cache::{CacheError, CacheInsert, CodeCache, CompiledArtifact};

pub(super) fn publish(
    cache: &mut CodeCache,
    #[allow(unused_mut)] mut artifact: CompiledArtifact,
) -> Result<CacheInsert, CacheError> {
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    artifact
        .publish_relocatable()
        .map_err(|_| CacheError::PublishFailed)?;
    cache.insert(artifact)
}
