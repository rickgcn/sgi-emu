#pragma once

class QDockWidget;
class QMainWindow;

namespace se::ui {

struct EmulationController;

QDockWidget* create_terminal_dock(
  QMainWindow* parent,
  const EmulationController& controller);

}
