{pkgs, ...}: {
  languages = {
    rust.enable = true;
  };

  packages = with pkgs; [
    rustPlatform.bindgenHook
    systemd
    libxcb
    pipewire
    alsa-lib
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
  ];
}
