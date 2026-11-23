//! The graphics API and card the window is drawn with, as the preferences ask (Settings ▸
//! Memory and GPU). Taken when the app starts, as wgpu can't change them under a window.

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
use eframe::wgpu;
use iskra_ui::log::Log;
use iskra_ui::prefs::Preferences;

/// eframe's wgpu setup, with the API and the card the preferences ask for. An API that finds
/// no card here gives way to the platform's own choice, so that the window still opens.
pub fn options(prefs: &Preferences, log: &Log) -> WgpuConfiguration {
    let mut options = WgpuConfiguration::default();
    let WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup else { return options };
    if let Some(backends) = prefs.graphics_backend.backends() {
        let found = pollster::block_on(
            wgpu::Instance::new(wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::new_without_display_handle() }).enumerate_adapters(backends),
        );
        if found.is_empty() {
            log.say(format!("graphics: {} finds no graphics card here, so the system's choice is drawn with", prefs.graphics_backend.label()));
        } else {
            setup.instance_descriptor.backends = backends;
        }
    }
    if let Some(card) = prefs.graphics_card.clone() {
        let (log, preference) = (log.clone(), setup.power_preference);
        setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
            let draws = |adapter: &&wgpu::Adapter| surface.is_none_or(|surface| adapter.is_surface_supported(surface));
            if let Some(named) = adapters.iter().filter(draws).find(|adapter| adapter.get_info().name == card) {
                return Ok(named.clone());
            }
            log.say(format!("graphics: the card {card} was not found, so the system's choice is drawn with"));
            // As wgpu would choose: the fast card where it was asked for, else the other.
            let fast = |adapter: &&wgpu::Adapter| {
                (adapter.get_info().device_type == wgpu::DeviceType::DiscreteGpu) == (preference == wgpu::PowerPreference::HighPerformance)
            };
            adapters
                .iter()
                .filter(draws)
                .find(fast)
                .or_else(|| adapters.iter().find(draws))
                .cloned()
                .ok_or_else(|| "no graphics card can draw the window".to_string())
        }));
    }
    options
}
