//! How CoreAudio says an audio device is connected (macOS).
//!
//! `cpal` does not pass this on: it reports every device except aggregates as
//! an unknown interface, so Bluetooth headsets cannot be told apart from
//! `cpal`'s description alone.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, NonNull};

use objc2_core_audio::{
    kAudioDevicePropertyTransportType, kAudioHardwarePropertyTranslateUIDToDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
};
use objc2_core_foundation::CFString;

/// Whether the CoreAudio device with this UID (the part of `cpal`'s device id
/// after `coreaudio:`) is connected over Bluetooth or Bluetooth LE. `None`
/// when CoreAudio does not know the UID or will not say.
pub(super) fn is_bluetooth(uid: &str) -> Option<bool> {
    let device = device_with_uid(uid)?;
    let transport: u32 = property(device, kAudioDevicePropertyTransportType, None)?;
    Some(super::device::is_bluetooth_transport(transport))
}

fn device_with_uid(uid: &str) -> Option<AudioObjectID> {
    let uid = CFString::from_str(uid);
    // The qualifier is a `CFStringRef`: CoreAudio is handed the address of one.
    let uid_ref: *const CFString = &*uid;
    let device: AudioObjectID = property(
        kAudioObjectSystemObject as AudioObjectID,
        kAudioHardwarePropertyTranslateUIDToDevice,
        Some((
            (&raw const uid_ref).cast(),
            size_of::<*const CFString>() as u32,
        )),
    )?;
    // CoreAudio answers "no such device" with the unknown object, 0.
    (device != 0).then_some(device)
}

/// Reads a global-scope property of `object` that is a plain `T`. `qualifier`
/// is the property's qualifier data and its size, for the properties that need one.
fn property<T: Default>(
    object: AudioObjectID,
    selector: u32,
    qualifier: Option<(*const c_void, u32)>,
) -> Option<T> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let (qualifier, qualifier_size) = qualifier.unwrap_or((null(), 0));
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `address`, `size` and `value` are live for the call, `value` is
    // `size` bytes, and `qualifier` is null or points at `qualifier_size`
    // bytes the caller keeps alive. The status is checked before `value` is used.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            qualifier_size,
            qualifier,
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0).then_some(value)
}
