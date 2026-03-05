import { sayHello } from "./helpers/test.mjs";

export function getCurrentTimestamp() {
    sayHello();
    console.log("Getting current timestamp...");
    console.log(new Date().toISOString());
    return [
        "ls",
        "pwd"
    ];
}