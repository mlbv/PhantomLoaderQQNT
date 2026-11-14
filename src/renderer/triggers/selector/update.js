// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-License-Identifier: MIT

const message = "更新进度与问题反馈请查看 PhantomLoaderQQNT 项目仓库。";

export default {
    hash: "#/setting",
    selector: ".config_view .versions .new setting-button",
    action(check_update_button) {
        let counting = 0;
        check_update_button.addEventListener("click", () => {
            counting++;
            if (counting == 20) {
                counting = 0;
                new Notification("PhantomLoaderQQNT", { body: message });
            }
        });
    }
}