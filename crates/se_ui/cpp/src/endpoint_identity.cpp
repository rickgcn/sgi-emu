#include "se_ui/endpoint_identity.h"

#include "se_ui/src/bridge.rs.h"

#include <cstddef>

namespace se_ui {

EndpointIdentity endpoint_identity(const EndpointHandleDto& handle) {
    return {
        handle.generation,
        std::string(handle.key.data(), static_cast<std::size_t>(handle.key.size())),
    };
}

EndpointHandleDto endpoint_handle_dto(const EndpointIdentity& identity) {
    return {
        identity.generation,
        rust::String(identity.key.data(), identity.key.size()),
    };
}

} // namespace se_ui
