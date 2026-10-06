#!/bin/bash
# Setup virtual audio devices on macOS using BlackHole
#
# This script configures BlackHole for E2E testing.
# Requires BlackHole to be installed: brew install blackhole-2ch
#
# Usage: ./setup-virtual-audio-macos.sh [check|create-aggregate]

set -euo pipefail

BLACKHOLE_NAME="BlackHole 2ch"

check_blackhole() {
    echo "Checking for BlackHole installation..."

    # Check if BlackHole is installed by looking for the audio device
    if system_profiler SPAudioDataType 2>/dev/null | grep -q "BlackHole"; then
        echo "BlackHole is installed and available"
        return 0
    else
        echo "BlackHole is not installed"
        echo ""
        echo "Install with Homebrew:"
        echo "  brew install blackhole-2ch"
        echo ""
        echo "Or download from: https://existential.audio/blackhole/"
        return 1
    fi
}

list_devices() {
    echo "Available audio devices:"
    echo ""
    system_profiler SPAudioDataType 2>/dev/null | grep -E "(Device Name|Manufacturer|Output Channels|Input Channels)" || true
}

create_aggregate_device() {
    echo "Creating aggregate audio device..."
    echo ""
    echo "NOTE: macOS aggregate devices must be created manually via Audio MIDI Setup."
    echo ""
    echo "Steps:"
    echo "1. Open 'Audio MIDI Setup' (Applications > Utilities)"
    echo "2. Click '+' button at bottom left"
    echo "3. Select 'Create Aggregate Device'"
    echo "4. Check both '$BLACKHOLE_NAME' and your built-in audio device"
    echo "5. Name it 'jamuru Test Device'"
    echo ""
    echo "For CI environments, use a pre-configured system or skip aggregate device."
}

set_default_device() {
    # This requires SwitchAudioSource utility
    if command -v SwitchAudioSource &> /dev/null; then
        echo "Setting BlackHole as default input..."
        SwitchAudioSource -t input -s "$BLACKHOLE_NAME" || true

        echo "Current audio configuration:"
        SwitchAudioSource -a
    else
        echo "SwitchAudioSource not found. Install with:"
        echo "  brew install switchaudio-osx"
        echo ""
        echo "Or manually set audio devices in System Preferences > Sound"
    fi
}

status() {
    echo "macOS Audio Status:"
    echo ""

    if check_blackhole; then
        echo ""
        list_devices
    fi

    echo ""
    if command -v SwitchAudioSource &> /dev/null; then
        echo "Current default devices:"
        echo "  Input:  $(SwitchAudioSource -c -t input 2>/dev/null || echo 'unknown')"
        echo "  Output: $(SwitchAudioSource -c -t output 2>/dev/null || echo 'unknown')"
    fi
}

usage() {
    echo "Usage: $0 [check|list|aggregate|default|status]"
    echo ""
    echo "Commands:"
    echo "  check     - Check if BlackHole is installed"
    echo "  list      - List available audio devices"
    echo "  aggregate - Instructions for creating aggregate device"
    echo "  default   - Set BlackHole as default input device"
    echo "  status    - Show current audio configuration"
}

case "${1:-status}" in
    check)
        check_blackhole
        ;;
    list)
        list_devices
        ;;
    aggregate)
        create_aggregate_device
        ;;
    default)
        set_default_device
        ;;
    status)
        status
        ;;
    *)
        usage
        exit 1
        ;;
esac
