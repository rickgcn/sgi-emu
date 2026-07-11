#pragma once

#include <cstdint>

#include "rust/cxx.h"

namespace se::ui {

struct EmulationController;

std::int32_t run_application(
  rust::Str version,
  rust::Vec<rust::String> arguments,
  const EmulationController& controller);

}
