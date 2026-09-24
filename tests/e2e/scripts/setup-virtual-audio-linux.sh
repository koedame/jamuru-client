#!/bin/bash
# Setup virtual audio devices on Linux using PipeWire
#
# This script creates virtual audio sink and source devices for E2E testing.
# Requires PipeWire and WirePlumber to be installed and running.
#
# Usage: ./setup-virtual-audio-linux.sh [create|destroy]

set -euo pipefail

SINK_NAME="jamjam-test-sink"
SOURCE_NAME="jamjam-test-source"
SINK_8CH_NAME="jamjam-test-8ch"

create_devices() {
    echo "Creating virtual audio devices..."

    # Check if PipeWire is running
    if ! pgrep -x "pipewire" > /dev/null; then
        echo "Error: PipeWire is not running"
        echo "Start PipeWire with: systemctl --user start pipewire pipewire-pulse"
        exit 1
    fi

    # Create virtual sink (for audio output capture)
    pw-cli create-node adapter '{
        factory.name = support.null-audio-sink
        node.name = "'"$SINK_NAME"'"
        node.description = "jamjam Test Sink"
        media.class = Audio/Sink
        audio.position = [ FL FR ]
        audio.rate = 48000
    }' 2>/dev/null || echo "Sink may already exist"

    # Create virtual source (for audio input injection)
    pw-cli create-node adapter '{
        factory.name = support.null-audio-sink
        node.name = "'"$SOURCE_NAME"'"
        node.description = "jamjam Test Source"
        media.class = Audio/Source
        audio.position = [ FL FR ]
        audio.rate = 48000
    }' 2>/dev/null || echo "Source may already exist"

    # Create an 8-channel sink that stands in for a multi-channel audio
    # interface (the GUI E2E scenarios about input/output channel settings)
    pw-cli create-node adapter '{
        factory.name = support.null-audio-sink
        node.name = "'"$SINK_8CH_NAME"'"
        node.description = "jamjam Test 8ch"
        media.class = Audio/Sink
        object.linger = true
        audio.channels = 8
        audio.position = [ FL FR FC LFE RL RR SL SR ]
        monitor.channel-volumes = true
        monitor.passthrough = true
    }' 2>/dev/null || echo "8ch sink may already exist"

    # Wait for nodes to be created
    sleep 1

    echo "Virtual audio devices created:"
    echo "  Sink: $SINK_NAME"
    echo "  Source: $SOURCE_NAME"
    echo "  8ch sink: $SINK_8CH_NAME"
    echo ""
    echo "The 8ch sink is reached through an ALSA PCM of the same name. Add this to"
    echo "~/.asoundrc, and run the app and the tests with"
    echo "PIPEWIRE_PROPS='{ stream.capture.sink=true }' so that a capture opened on a"
    echo "sink reads that sink's monitor:"
    echo ""
    echo "  pcm.$SINK_8CH_NAME {"
    echo "    type pipewire"
    echo "    playback_node \"$SINK_8CH_NAME\""
    echo "    capture_node \"$SINK_8CH_NAME\""
    echo "    hint { show on description \"$SINK_8CH_NAME\" }"
    echo "  }"

    # List created devices
    echo ""
    echo "Available devices:"
    pw-cli list-objects Node | grep -E "(jamjam|null-audio)" || true
}

destroy_devices() {
    echo "Destroying virtual audio devices..."

    # Find and destroy the sink
    SINK_ID=$(pw-cli list-objects Node | grep -B5 "$SINK_NAME" | grep "id:" | awk '{print $2}' | tr -d ',')
    if [ -n "$SINK_ID" ]; then
        pw-cli destroy "$SINK_ID" 2>/dev/null || true
        echo "Destroyed sink (id: $SINK_ID)"
    fi

    # Find and destroy the 8ch sink
    SINK_8CH_ID=$(pw-cli list-objects Node | grep -B5 "$SINK_8CH_NAME" | grep "id:" | awk '{print $2}' | tr -d ',')
    if [ -n "$SINK_8CH_ID" ]; then
        pw-cli destroy "$SINK_8CH_ID" 2>/dev/null || true
        echo "Destroyed 8ch sink (id: $SINK_8CH_ID)"
    fi

    # Find and destroy the source
    SOURCE_ID=$(pw-cli list-objects Node | grep -B5 "$SOURCE_NAME" | grep "id:" | awk '{print $2}' | tr -d ',')
    if [ -n "$SOURCE_ID" ]; then
        pw-cli destroy "$SOURCE_ID" 2>/dev/null || true
        echo "Destroyed source (id: $SOURCE_ID)"
    fi

    echo "Virtual audio devices destroyed"
}

link_loopback() {
    echo "Creating loopback link (sink monitor -> source input)..."

    # Link sink's monitor output to source's input for loopback testing
    pw-link "${SINK_NAME}:monitor_FL" "${SOURCE_NAME}:input_FL" 2>/dev/null || echo "FL link may already exist"
    pw-link "${SINK_NAME}:monitor_FR" "${SOURCE_NAME}:input_FR" 2>/dev/null || echo "FR link may already exist"

    echo "Loopback link created"
}

status() {
    echo "Virtual audio device status:"
    echo ""
    echo "PipeWire nodes:"
    pw-cli list-objects Node | grep -E "(jamjam|null-audio)" || echo "No jamjam devices found"
    echo ""
    echo "PipeWire links:"
    pw-link -l 2>/dev/null | grep -E "(jamjam|test)" || echo "No jamjam links found"
}

usage() {
    echo "Usage: $0 [create|destroy|loopback|status]"
    echo ""
    echo "Commands:"
    echo "  create   - Create virtual audio devices"
    echo "  destroy  - Destroy virtual audio devices"
    echo "  loopback - Create loopback link between sink and source"
    echo "  status   - Show current virtual audio device status"
}

case "${1:-create}" in
    create)
        create_devices
        ;;
    destroy)
        destroy_devices
        ;;
    loopback)
        link_loopback
        ;;
    status)
        status
        ;;
    *)
        usage
        exit 1
        ;;
esac
