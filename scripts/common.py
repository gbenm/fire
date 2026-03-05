from typing import List

def sayHello(first_name: str, last_name: str, *args: List[str]) -> None:
    extras = " " + " ".join(str(a) for a in args) if args else ""
    print(f"Hello, {first_name} {last_name}!{extras}")
    return [
        "ls",
        "pwd"
    ]
