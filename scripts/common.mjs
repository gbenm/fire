import { sayHello } from "./helpers/test.mjs";

export function getCurrentTimestamp() {
    sayHello();
    console.log("Getting current timestamp...");
    return new Date().toISOString();
}