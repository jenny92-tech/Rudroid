//! INPUT:  Host API named-service registry
//! OUTPUT:  typed query for `rudroid.unity.mono.v1`
//! POS:    zero-unsafe consumer of the Mono service contract
#![forbid(unsafe_code)]

use rudroid_common::host_api::HostApiV0;
use rudroid_unity_common::mono_abi::MonoServiceV1;

pub(crate) fn query(api: &HostApiV0) -> Option<&'static MonoServiceV1> {
    api.query_service_typed::<MonoServiceV1>()
}
