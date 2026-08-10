# Immagine per compilare/pacchettizzare Rclone Easy in locale, senza
# passare da GitHub Actions. Base Ubuntu 22.04 non a caso: è la stessa
# usata in .github/workflows/build-linux.yml, dove il bundling AppImage
# funziona — su un rolling release come CachyOS lo strip incluso in
# linuxdeploy non riconosce sezioni ELF recenti (.relr.dyn) prodotte dal suo
# toolchain, facendo fallire l'AppImage (vedi commenti nel workflow). Una
# base più vecchia e conservativa evita il problema alla radice.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

# Stessa lista di .github/workflows/build-linux.yml (tenerle allineate a
# mano se cambia una delle due), più curl/unzip/ca-certificates/git che lì
# sono già presenti sui runner GitHub ma qui vanno installati esplicitamente.
RUN apt-get update && apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    patchelf \
    build-essential \
    curl \
    wget \
    file \
    libssl-dev \
    libgtk-3-dev \
    unzip \
    ca-certificates \
    git \
    xdg-utils \
    && rm -rf /var/lib/apt/lists/*

# Node.js (stessa major usata in CI)
RUN curl -fsSL https://deb.nodesource.com/setup_24.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# Rust
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal

WORKDIR /workspace
