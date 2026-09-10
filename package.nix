{
  lib,
  rustPlatform,
  pkg-config,
  udev,
}:

rustPlatform.buildRustPackage {
  pname = "glor";
  version = "0.1.0";

  src = lib.cleanSource ./.;
  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ pkg-config ];
  # hidapi's hidraw backend links against libudev.
  buildInputs = [ udev ];

  # Tests are enabled through flake checks.
  doCheck = false;

  postInstall = ''
    install -Dm444 udev/70-glorious.rules \
      $out/lib/udev/rules.d/70-glorious.rules
  '';

  meta = {
    description = "Configure Pixart-based Glorious mice (Model O 2 / I 2 family) on Linux";
    homepage = "https://github.com/lccpianoman/glor";
    license = lib.licenses.mit;
    mainProgram = "glor";
    platforms = lib.platforms.linux;
  };
}
