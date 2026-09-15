#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <string>

namespace se_ui {

struct EndpointHandleDto;

struct EndpointIdentity {
    std::uint64_t generation = 0;
    std::string key;

    [[nodiscard]] bool operator==(const EndpointIdentity& other) const {
        return generation == other.generation && key == other.key;
    }
};

[[nodiscard]] EndpointIdentity endpoint_identity(const EndpointHandleDto& handle);
[[nodiscard]] EndpointHandleDto endpoint_handle_dto(const EndpointIdentity& identity);

} // namespace se_ui
