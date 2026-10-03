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
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-plugins-ugly
  ];
}
