// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-License-Identifier: MIT

import { PhantomLoader } from "../../api.js";
import { BaseElement } from "../element.js";


export class Link extends BaseElement {
    #openExternalBound = this.#openExternal.bind(this);

    connectedCallback() {
        super.connectedCallback();
        this.addEventListener("click", this.#openExternalBound);
    }

    disconnectedCallback() {
        super.disconnectedCallback();
        this.removeEventListener("click", this.#openExternalBound);
    }

    #openExternal() {
        const value = this.getValue();
        try {
            new URL(value);
            PhantomLoader.api.openExternal(value);
        } catch {
            PhantomLoader.api.openPath(value);
        }
    }

    update() {
        this.textContent ||= this.getValue();
    }

    getTemplate() {
        return /*html*/ `
            <slot></slot>
        `;
    }

    getStyles() {
        return /*css*/ `
            :host { color: var(--text_link); cursor: pointer; }
        `;
    }
}
