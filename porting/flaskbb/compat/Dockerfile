FROM python:3.7.17-bullseye@sha256:f36f6fe9ddb2622aab9803944cbe7e2bc50be75d624a081f7895e60e7c6b7f0a
ENV PYTHONDONTWRITEBYTECODE=1 PIP_DISABLE_PIP_VERSION_CHECK=1
WORKDIR /app
COPY porting/flaskbb/upstream/ /app/
COPY porting/flaskbb/compat/requirements.extra.txt /tmp/requirements.extra.txt
RUN python -m pip install --upgrade 'pip<24' 'setuptools<68' 'wheel<0.43' \
    && sed -i '/^git+https:\/\/github.com\/justanr\/flask-allows.git/d' requirements.txt \
    && pip install -r requirements.txt -r /tmp/requirements.extra.txt \
    && pip install -e . --no-deps
EXPOSE 5000
CMD ["flaskbb", "--config", "/runtime/flaskbb.cfg", "run", "--host", "0.0.0.0"]
