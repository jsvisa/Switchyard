# SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

.PHONY: install-macos install-macos-dry-run uninstall-macos

## Install the Switchyard background server and menu bar app for this user.
install-macos:
	@scripts/macos/install.sh

## Print what install-macos would do, without changing anything.
install-macos-dry-run:
	@scripts/macos/install.sh --dry-run

## Remove the LaunchAgents, the sy Codex profile, and the codex alias.
uninstall-macos:
	@scripts/macos/uninstall.sh
