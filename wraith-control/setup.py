from setuptools import setup, find_packages

setup(
    name="wraith-control",
    version="0.1.0",
    description="Wraith Process Teleportation Engine — orchestration and CLI",
    long_description=open("README.md").read() if __import__("os").path.exists("README.md") else "",
    packages=find_packages(exclude=["tests*", "examples*"]),
    python_requires=">=3.10",
    install_requires=[
        "click>=8.1",
        "paramiko>=3.0",
    ],
    extras_require={
        "dev": [
            "pytest>=7.0",
            "pytest-mock>=3.10",
        ],
    },
    entry_points={
        "console_scripts": [
            "wraith=wraith.cli:cli",
        ],
    },
    classifiers=[
        "Development Status :: 3 - Alpha",
        "Intended Audience :: System Administrators",
        "Operating System :: POSIX :: Linux",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
    ],
)
