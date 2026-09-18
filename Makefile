PY ?= python3
export PYTHONPATH := src

.PHONY: install demo atlas smoke clean

install:
	$(PY) -m pip install -r requirements.txt

atlas:            ## render the nine prebuilt maps
	$(PY) -m tolmap.cli render $(patsubst data/%.json,%,$(wildcard data/*.json)) \
	  --out data -o atlas.html

demo: atlas
	@echo "open atlas.html"

smoke:            ## index one repo end to end: make smoke REPO=~/src/flask PKG=src/flask
	$(PY) -m tolmap.cli build $(REPO) --pkg $(PKG) --lang $(or $(LANG),py) \
	  --name smoke --out /tmp/tolmap-smoke
	$(PY) -m tolmap.cli render smoke --out /tmp/tolmap-smoke -o /tmp/tolmap-smoke/atlas.html

clean:
	rm -rf out atlas.html __pycache__ src/tolmap/__pycache__
