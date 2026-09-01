//! Top-level ownership of the runtime during a graphical session.

use se_cpu::mips1::r3000::debug::{
    CacheView, PendingCp0DebugSnapshot, PendingCp1DebugSnapshot, TlbView,
};
use se_machine::debug::{DebugRequest, DebugResponse};
use se_machine::indigo::ip12::debug::{
    DebugRequest as Ip12DebugRequest, DebugResponse as Ip12DebugResponse, MemoryAddressSpace,
};
use se_machine::machine::Machine;
use se_machine::output::MachineOutput;
use se_machine::serial::SerialPort;
use se_runtime::control::{RuntimeState, RuntimeStatus};
use se_runtime::runtime::{DebugReply, Runtime, RuntimeError, ShutdownError};

use crate::bridge::ffi::{
    CacheDto, CacheEntryDto, DisassemblyDto, DisassemblyLineDto, MachineOutputSink, MemoryDto,
    RegistersDto, RuntimeStatusDto, SerialPortDto, TlbDto, TlbEntryDto, UiExitState,
    UiStartupState, run_gui,
};

/// Constructs a machine from settings selected by a frontend.
pub type MachineBuilder =
    Box<dyn Fn(&str, &str, &str) -> Result<Machine, String> + Send + Sync + 'static>;

/// Owns the emulator runtime for the lifetime of one Qt event loop.
pub struct UiSession {
    runtime: Option<Runtime>,
    machine_builder: MachineBuilder,
}

impl UiSession {
    /// Creates a session around a runtime and a machine-construction function.
    #[must_use]
    pub fn new(runtime: Runtime, machine_builder: MachineBuilder) -> Self {
        Self {
            runtime: Some(runtime),
            machine_builder,
        }
    }

    /// Runs the graphical user interface until its main window closes.
    pub fn run(&self, startup: &UiStartupState) -> UiExitState {
        run_gui(self, startup)
    }

    /// Stops the runtime worker and waits for it to exit.
    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        match self.runtime.take() {
            Some(runtime) => runtime.shutdown(),
            None => Ok(()),
        }
    }

    /// Samples current runtime status for Qt.
    pub fn runtime_status(&self) -> RuntimeStatusDto {
        self.runtime_command(Runtime::status)
    }

    /// Builds and installs a machine selected in the settings dialog.
    pub fn configure_machine(
        &self,
        model: &str,
        prom_path: &str,
        float_backend: &str,
    ) -> RuntimeStatusDto {
        let machine = match (self.machine_builder)(model, prom_path, float_backend) {
            Ok(machine) => machine,
            Err(error) => return failed_status(error),
        };
        self.runtime_command(|runtime| runtime.configure(machine))
    }

    /// Starts continuous machine execution.
    pub fn run_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(Runtime::run)
    }

    /// Resets and pauses the configured machine.
    pub fn reset_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(Runtime::reset)
    }

    /// Pauses continuous machine execution.
    pub fn pause_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(Runtime::pause)
    }

    /// Executes one instruction while paused.
    pub fn step_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(Runtime::step)
    }

    /// Connects runtime machine output to the Qt delivery sink.
    pub fn attach_machine_output(
        &self,
        sink: cxx::SharedPtr<MachineOutputSink>,
    ) -> RuntimeStatusDto {
        if sink.is_null() {
            return failed_status(String::from("machine output sink is unavailable"));
        }

        self.runtime_command(|runtime| {
            runtime.set_output_handler(Box::new(move |output: MachineOutput| {
                sink.publish_serial(output.serial(SerialPort::A), output.serial(SerialPort::B));
            }))
        })
    }

    /// Disconnects the Qt delivery sink from the runtime worker.
    pub fn detach_machine_output(&self) {
        if let Some(runtime) = self.runtime.as_ref() {
            let _ = runtime.clear_output_handler();
        }
    }

    /// Samples processor registers and pending effects.
    pub fn registers(&self) -> RegistersDto {
        let reply = match self.debug(Ip12DebugRequest::Registers) {
            Ok(reply) => reply,
            Err(error) => return failed_registers(error.to_string()),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Registers(snapshot)) = reply.response
        else {
            return failed_registers(String::from("runtime returned an unexpected response"));
        };
        let cpu = snapshot.cpu;
        let pending_effective = cpu.cp0.pending_functional.map_or_else(Vec::new, |state| {
            vec![
                state.coprocessor_usable,
                state.interrupt_control,
                state.software_interrupts,
            ]
        });

        RegistersDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            pc: cpu.pc,
            hi: cpu.hi,
            lo: cpu.lo,
            gpr: cpu.gpr.into(),
            delay_slot: cpu.delay_slot.map_or_else(
                || String::from("none"),
                |slot| format!("0x{:08x} -> 0x{:08x}", slot.origin_pc, slot.resume_pc),
            ),
            pending_gpr: cpu.pending_gpr.map_or_else(
                || String::from("none"),
                |write| format!("${} = 0x{:08x}", write.index, write.value),
            ),
            pending_cp0: format_pending_cp0(cpu.pending_cp0),
            pending_cp1: format_pending_cp1(cpu.pending_cp1),
            cp0: cpu.cp0.registers.into(),
            cp0_effective: vec![
                cpu.cp0.effective.coprocessor_usable,
                cpu.cp0.effective.interrupt_control,
                cpu.cp0.effective.software_interrupts,
            ],
            cp0_pending_effective: pending_effective,
            cp1: cpu.cp1.registers.into(),
            fcr0: cpu.cp1.fcr0,
            fcr30: cpu.cp1.fcr30,
            fcr31: cpu.cp1.fcr31,
            float_backend: format!("{:?}", cpu.cp1.backend),
            cp1_interrupt: cpu.cp1.interrupt_asserted,
        }
    }

    /// Samples one TLB view.
    pub fn tlb(&self, instruction_view: bool) -> TlbDto {
        let view = if instruction_view {
            TlbView::Instruction
        } else {
            TlbView::Main
        };
        let reply = match self.debug(Ip12DebugRequest::Tlb(view)) {
            Ok(reply) => reply,
            Err(error) => return failed_tlb(error.to_string(), instruction_view),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Tlb(snapshot)) = reply.response else {
            return failed_tlb(
                String::from("runtime returned an unexpected response"),
                instruction_view,
            );
        };

        TlbDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            instruction_view,
            shutdown: snapshot.shutdown,
            index: snapshot.index as u32,
            random: snapshot.random as u32,
            entries: snapshot
                .entries
                .into_iter()
                .map(|entry| TlbEntryDto {
                    index: entry.index as u32,
                    entry_hi: entry.entry_hi,
                    entry_lo: entry.entry_lo,
                    vpn: entry.vpn,
                    asid: entry.asid,
                    pfn: entry.pfn,
                    noncacheable: entry.noncacheable,
                    dirty: entry.dirty,
                    valid: entry.valid,
                    global: entry.global,
                })
                .collect(),
        }
    }

    /// Samples one physical cache bank.
    pub fn cache(&self, instruction_cache: bool) -> CacheDto {
        let view = if instruction_cache {
            CacheView::Instruction
        } else {
            CacheView::Data
        };
        let reply = match self.debug(Ip12DebugRequest::Cache(view)) {
            Ok(reply) => reply,
            Err(error) => return failed_cache(error.to_string(), instruction_cache),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Cache(snapshot)) = reply.response else {
            return failed_cache(
                String::from("runtime returned an unexpected response"),
                instruction_cache,
            );
        };

        CacheDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            instruction_cache,
            refill_bytes: snapshot.refill_bytes as u32,
            entries: snapshot
                .entries
                .into_iter()
                .map(|entry| CacheEntryDto {
                    index: entry.index as u32,
                    page_frame: entry.page_frame,
                    word: entry.word,
                    valid: entry.valid,
                })
                .collect(),
        }
    }

    /// Reads and disassembles virtual instructions.
    pub fn disassembly(&self, start: u32, row_count: u32) -> DisassemblyDto {
        let reply = match self.debug(Ip12DebugRequest::Disassembly {
            start,
            row_count: row_count as usize,
        }) {
            Ok(reply) => reply,
            Err(error) => return failed_disassembly(error.to_string()),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Disassembly(lines)) = reply.response
        else {
            return failed_disassembly(String::from("runtime returned an unexpected response"));
        };
        let current_pc = reply.execution_address;

        DisassemblyDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            lines: lines
                .into_iter()
                .map(|line| DisassemblyLineDto {
                    address: line.address,
                    readable: line.word.is_some(),
                    word: line.word.unwrap_or(0),
                    text: line.text,
                    current: current_pc == line.address,
                    breakpoint: reply.breakpoints.binary_search(&line.address).is_ok(),
                })
                .collect(),
        }
    }

    /// Reads a side-effect-free physical or virtual memory range.
    pub fn memory(&self, virtual_address_space: bool, start: u64, length: u32) -> MemoryDto {
        let address_space = if virtual_address_space {
            MemoryAddressSpace::Virtual
        } else {
            MemoryAddressSpace::Physical
        };
        let reply = match self.debug(Ip12DebugRequest::Memory {
            address_space,
            start,
            length: length as usize,
        }) {
            Ok(reply) => reply,
            Err(error) => {
                return failed_memory(error.to_string(), virtual_address_space, start);
            }
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Memory(snapshot)) = reply.response else {
            return failed_memory(
                String::from("runtime returned an unexpected response"),
                virtual_address_space,
                start,
            );
        };
        let mut values = Vec::with_capacity(snapshot.bytes.len());
        let mut readable = Vec::with_capacity(snapshot.bytes.len());
        for byte in snapshot.bytes {
            values.push(byte.unwrap_or(0));
            readable.push(u8::from(byte.is_some()));
        }

        MemoryDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            virtual_address_space,
            start,
            values,
            readable,
        }
    }

    /// Adds or removes one virtual execution breakpoint.
    pub fn toggle_breakpoint(&self, address: u32) -> RuntimeStatusDto {
        let Some(runtime) = self.runtime.as_ref() else {
            return failed_status(String::from("runtime is unavailable"));
        };
        match runtime.toggle_breakpoint(address) {
            Ok(status) => status_dto(status),
            Err(error) => failed_status(error.to_string()),
        }
    }

    /// Supplies one byte batch to an external serial port.
    pub fn send_serial(&self, port: SerialPortDto, bytes: &[u8]) -> RuntimeStatusDto {
        let port = match port {
            SerialPortDto::A => SerialPort::A,
            SerialPortDto::B => SerialPort::B,
            _ => return failed_status(String::from("unsupported serial port")),
        };
        self.runtime_command(|runtime| runtime.send_serial(port, bytes))
    }

    fn runtime_command(
        &self,
        command: impl FnOnce(&Runtime) -> Result<RuntimeStatus, RuntimeError>,
    ) -> RuntimeStatusDto {
        let Some(runtime) = self.runtime.as_ref() else {
            return failed_status(String::from("runtime is unavailable"));
        };
        match command(runtime) {
            Ok(status) => status_dto(status),
            Err(error) => failed_status(error.to_string()),
        }
    }

    fn debug(&self, request: Ip12DebugRequest) -> Result<DebugReply, RuntimeError> {
        self.runtime
            .as_ref()
            .ok_or(RuntimeError::WorkerUnavailable)?
            .debug(DebugRequest::IndigoIp12(request))
    }
}

fn status_dto(status: RuntimeStatus) -> RuntimeStatusDto {
    RuntimeStatusDto {
        success: true,
        state: state_identifier(status.state),
        revision: status.revision,
        execution_error: status.last_error.unwrap_or_default(),
        command_error: String::new(),
    }
}

const fn state_identifier(state: RuntimeState) -> u8 {
    match state {
        RuntimeState::Unconfigured => 0,
        RuntimeState::Paused => 1,
        RuntimeState::Running => 2,
    }
}

fn failed_status(error: String) -> RuntimeStatusDto {
    RuntimeStatusDto {
        success: false,
        state: 0,
        revision: 0,
        execution_error: String::new(),
        command_error: error,
    }
}

fn failed_registers(error: String) -> RegistersDto {
    RegistersDto {
        success: false,
        error,
        revision: 0,
        pc: 0,
        hi: 0,
        lo: 0,
        gpr: Vec::new(),
        delay_slot: String::new(),
        pending_gpr: String::new(),
        pending_cp0: String::new(),
        pending_cp1: String::new(),
        cp0: Vec::new(),
        cp0_effective: Vec::new(),
        cp0_pending_effective: Vec::new(),
        cp1: Vec::new(),
        fcr0: 0,
        fcr30: 0,
        fcr31: 0,
        float_backend: String::new(),
        cp1_interrupt: false,
    }
}

fn failed_tlb(error: String, instruction_view: bool) -> TlbDto {
    TlbDto {
        success: false,
        error,
        revision: 0,
        instruction_view,
        shutdown: false,
        index: 0,
        random: 0,
        entries: Vec::new(),
    }
}

fn failed_cache(error: String, instruction_cache: bool) -> CacheDto {
    CacheDto {
        success: false,
        error,
        revision: 0,
        instruction_cache,
        refill_bytes: 0,
        entries: Vec::new(),
    }
}

fn failed_disassembly(error: String) -> DisassemblyDto {
    DisassemblyDto {
        success: false,
        error,
        revision: 0,
        lines: Vec::new(),
    }
}

fn failed_memory(error: String, virtual_address_space: bool, start: u64) -> MemoryDto {
    MemoryDto {
        success: false,
        error,
        revision: 0,
        virtual_address_space,
        start,
        values: Vec::new(),
        readable: Vec::new(),
    }
}

fn format_pending_cp0(pending: Option<PendingCp0DebugSnapshot>) -> String {
    match pending {
        None => String::from("none"),
        Some(PendingCp0DebugSnapshot::Register { index, value }) => {
            format!("${index} = 0x{value:08x}")
        }
        Some(PendingCp0DebugSnapshot::TlbRead { entry_hi, entry_lo }) => {
            format!("TLBR: EntryHi=0x{entry_hi:08x}, EntryLo=0x{entry_lo:08x}")
        }
        Some(PendingCp0DebugSnapshot::TlbProbe { index }) => {
            format!("TLBP: Index=0x{index:08x}")
        }
    }
}

fn format_pending_cp1(pending: Option<PendingCp1DebugSnapshot>) -> String {
    match pending {
        None => String::from("none"),
        Some(PendingCp1DebugSnapshot::General { index, value }) => {
            format!("$f{index} = 0x{value:08x}")
        }
        Some(PendingCp1DebugSnapshot::Control { index, value }) => {
            format!("FCR{index} = 0x{value:08x}")
        }
        Some(PendingCp1DebugSnapshot::Condition { value }) => {
            format!("condition = {}", u8::from(value))
        }
    }
}
